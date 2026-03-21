use anyhow::{bail, Result};
use serde_json::json;

use crate::transform::analyze::WorkloadAnalysis;
use crate::transform::planner::LlmProvider;

use super::context::PgContext;
use super::types::{Recommendation, TuningIteration};

/// Trait for LLM-based tuning advisors.
#[async_trait::async_trait]
pub trait TuningAdvisor: Send + Sync {
    async fn recommend(
        &self,
        context: &PgContext,
        workload: &WorkloadAnalysis,
        hint: Option<&str>,
        previous: &[TuningIteration],
    ) -> Result<Vec<Recommendation>>;

    fn name(&self) -> &str;
}

/// Configuration for creating a TuningAdvisor.
pub struct AdvisorConfig {
    pub provider: LlmProvider,
    pub api_key: String,
    pub api_url: Option<String>,
    pub model: Option<String>,
}

/// Create a TuningAdvisor from config.
pub fn create_advisor(config: AdvisorConfig) -> Box<dyn TuningAdvisor> {
    match config.provider {
        LlmProvider::Claude => {
            let model = config
                .model
                .unwrap_or_else(|| "claude-sonnet-4-20250514".into());
            let url = config
                .api_url
                .unwrap_or_else(|| "https://api.anthropic.com".into());
            Box::new(ClaudeAdvisor {
                api_key: config.api_key,
                model,
                base_url: url,
            })
        }
        LlmProvider::OpenAi => {
            let model = config.model.unwrap_or_else(|| "gpt-4o".into());
            let url = config
                .api_url
                .unwrap_or_else(|| "https://api.openai.com".into());
            Box::new(OpenAiAdvisor {
                api_key: config.api_key,
                model,
                base_url: url,
            })
        }
        LlmProvider::Gemini => {
            let model = config.model.unwrap_or_else(|| "gemini-2.5-flash".into());
            let url = config
                .api_url
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into());
            Box::new(GeminiAdvisor {
                api_key: config.api_key,
                model,
                base_url: url,
            })
        }
        LlmProvider::Bedrock => {
            let model = config
                .model
                .unwrap_or_else(|| "us.anthropic.claude-sonnet-4-20250514-v1:0".into());
            Box::new(BedrockAdvisor {
                model,
                region: config.api_url, // reuse api_url as region override
            })
        }
        LlmProvider::Ollama => {
            let model = config.model.unwrap_or_else(|| "llama3".into());
            let url = config
                .api_url
                .unwrap_or_else(|| "http://localhost:11434".into());
            Box::new(OllamaAdvisor {
                model,
                base_url: url,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt construction
// ---------------------------------------------------------------------------

pub fn build_system_prompt() -> String {
    r#"You are a PostgreSQL tuning expert. Given a database's current configuration, schema, query performance, and workload patterns, recommend changes to improve performance.

You have four recommendation tools:
1. config_change — Change a PostgreSQL configuration parameter
2. create_index — Create a new index to speed up queries
3. query_rewrite — Suggest an optimized version of a slow query
4. schema_change — Suggest a schema modification (ALTER TABLE, etc.)

Guidelines:
- Prioritize changes with the highest expected performance impact
- For config changes, specify the parameter name, current value, and recommended value
- For indexes, provide the complete CREATE INDEX statement
- For query rewrites, show the original and optimized SQL side by side
- Consider previous iteration results — do not repeat changes that were ineffective
- Provide clear rationale for each recommendation
- Limit to 3-5 recommendations per iteration (focus on highest impact)"#.into()
}

pub fn build_user_message(
    context: &PgContext,
    workload: &WorkloadAnalysis,
    hint: Option<&str>,
    previous: &[TuningIteration],
) -> String {
    let mut msg = String::new();

    msg.push_str("## Database Context\n");
    msg.push_str(&serde_json::to_string_pretty(context).unwrap_or_default());
    msg.push_str("\n\n");

    msg.push_str("## Workload Summary\n");
    msg.push_str(&serde_json::to_string_pretty(workload).unwrap_or_default());
    msg.push_str("\n\n");

    if let Some(h) = hint {
        msg.push_str("## User Hint\n");
        msg.push_str(h);
        msg.push_str("\n\n");
    }

    if !previous.is_empty() {
        msg.push_str("## Previous Iterations\n");
        for iter in previous {
            msg.push_str(&format!(
                "Iteration {}: {} recommendations applied.\n",
                iter.iteration,
                iter.applied.iter().filter(|a| a.success).count()
            ));
            msg.push_str(&iter.llm_feedback);
            msg.push('\n');
        }
        msg.push('\n');
    }

    msg.push_str("## Instructions\n");
    msg.push_str("Analyze the database context and workload, then use the tools to recommend tuning changes.\n");

    msg
}

fn tool_schema() -> serde_json::Value {
    json!([
        {
            "name": "config_change",
            "description": "Recommend a PostgreSQL configuration parameter change",
            "input_schema": {
                "type": "object",
                "properties": {
                    "parameter": { "type": "string", "description": "PostgreSQL parameter name" },
                    "current_value": { "type": "string", "description": "Current value" },
                    "recommended_value": { "type": "string", "description": "Recommended new value" },
                    "rationale": { "type": "string", "description": "Why this change helps" }
                },
                "required": ["parameter", "current_value", "recommended_value", "rationale"]
            }
        },
        {
            "name": "create_index",
            "description": "Recommend creating a new database index",
            "input_schema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string" },
                    "columns": { "type": "array", "items": { "type": "string" } },
                    "index_type": { "type": "string", "description": "btree, hash, gin, gist (optional)" },
                    "sql": { "type": "string", "description": "Complete CREATE INDEX statement" },
                    "rationale": { "type": "string" }
                },
                "required": ["table", "columns", "sql", "rationale"]
            }
        },
        {
            "name": "query_rewrite",
            "description": "Suggest an optimized version of a slow query",
            "input_schema": {
                "type": "object",
                "properties": {
                    "original_sql": { "type": "string" },
                    "rewritten_sql": { "type": "string" },
                    "rationale": { "type": "string" }
                },
                "required": ["original_sql", "rewritten_sql", "rationale"]
            }
        },
        {
            "name": "schema_change",
            "description": "Suggest a schema modification",
            "input_schema": {
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "ALTER TABLE or other DDL" },
                    "description": { "type": "string" },
                    "rationale": { "type": "string" }
                },
                "required": ["sql", "description", "rationale"]
            }
        }
    ])
}

fn parse_tool_call(name: &str, input: &serde_json::Value) -> Option<Recommendation> {
    match name {
        "config_change" => Some(Recommendation::ConfigChange {
            parameter: input["parameter"].as_str()?.into(),
            current_value: input["current_value"].as_str()?.into(),
            recommended_value: input["recommended_value"].as_str()?.into(),
            rationale: input["rationale"].as_str().unwrap_or("").into(),
        }),
        "create_index" => Some(Recommendation::CreateIndex {
            table: input["table"].as_str()?.into(),
            columns: input["columns"]
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            index_type: input["index_type"].as_str().map(String::from),
            sql: input["sql"].as_str()?.into(),
            rationale: input["rationale"].as_str().unwrap_or("").into(),
        }),
        "query_rewrite" => Some(Recommendation::QueryRewrite {
            original_sql: input["original_sql"].as_str()?.into(),
            rewritten_sql: input["rewritten_sql"].as_str()?.into(),
            rationale: input["rationale"].as_str().unwrap_or("").into(),
        }),
        "schema_change" => Some(Recommendation::SchemaChange {
            sql: input["sql"].as_str()?.into(),
            description: input["description"].as_str().unwrap_or("").into(),
            rationale: input["rationale"].as_str().unwrap_or("").into(),
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Claude Advisor
// ---------------------------------------------------------------------------

struct ClaudeAdvisor {
    api_key: String,
    model: String,
    base_url: String,
}

#[async_trait::async_trait]
impl TuningAdvisor for ClaudeAdvisor {
    async fn recommend(
        &self,
        context: &PgContext,
        workload: &WorkloadAnalysis,
        hint: Option<&str>,
        previous: &[TuningIteration],
    ) -> Result<Vec<Recommendation>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": build_system_prompt(),
            "tools": tool_schema(),
            "messages": [{ "role": "user", "content": build_user_message(context, workload, hint, previous) }]
        });

        let resp = client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Claude API error {status}: {text}");
        }

        let data: serde_json::Value = resp.json().await?;
        let mut recs = Vec::new();

        if let Some(content) = data["content"].as_array() {
            for block in content {
                if block["type"] == "tool_use" {
                    if let Some(name) = block["name"].as_str() {
                        if let Some(rec) = parse_tool_call(name, &block["input"]) {
                            recs.push(rec);
                        }
                    }
                }
            }
        }

        Ok(recs)
    }

    fn name(&self) -> &str {
        "Claude"
    }
}

// ---------------------------------------------------------------------------
// OpenAI Advisor
// ---------------------------------------------------------------------------

struct OpenAiAdvisor {
    api_key: String,
    model: String,
    base_url: String,
}

#[async_trait::async_trait]
impl TuningAdvisor for OpenAiAdvisor {
    async fn recommend(
        &self,
        context: &PgContext,
        workload: &WorkloadAnalysis,
        hint: Option<&str>,
        previous: &[TuningIteration],
    ) -> Result<Vec<Recommendation>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let schema = tool_schema();
        let tools: Vec<serde_json::Value> = schema
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("tool_schema did not return an array"))?
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t["name"],
                        "description": t["description"],
                        "parameters": t["input_schema"]
                    }
                })
            })
            .collect();

        // Newer OpenAI models (gpt-5, o-series) use max_completion_tokens;
        // older models (gpt-4o, gpt-4) use max_tokens
        let uses_new_param = self.model.starts_with("gpt-5")
            || self.model.starts_with("o1")
            || self.model.starts_with("o3")
            || self.model.starts_with("o4");
        let token_key = if uses_new_param {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };

        let mut body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": build_system_prompt() },
                { "role": "user", "content": build_user_message(context, workload, hint, previous) }
            ],
            "tools": tools
        });
        body[token_key] = json!(4096);

        let resp = client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("OpenAI API error {status}: {text}");
        }

        let data: serde_json::Value = resp.json().await?;
        let mut recs = Vec::new();

        if let Some(tool_calls) = data["choices"][0]["message"]["tool_calls"].as_array() {
            for tc in tool_calls {
                if let (Some(name), Some(args_str)) = (
                    tc["function"]["name"].as_str(),
                    tc["function"]["arguments"].as_str(),
                ) {
                    if let Ok(args) = serde_json::from_str::<serde_json::Value>(args_str) {
                        if let Some(rec) = parse_tool_call(name, &args) {
                            recs.push(rec);
                        }
                    }
                }
            }
        }

        Ok(recs)
    }

    fn name(&self) -> &str {
        "OpenAI"
    }
}

// ---------------------------------------------------------------------------
// Gemini Advisor
// ---------------------------------------------------------------------------

struct GeminiAdvisor {
    api_key: String,
    model: String,
    base_url: String,
}

#[async_trait::async_trait]
impl TuningAdvisor for GeminiAdvisor {
    async fn recommend(
        &self,
        context: &PgContext,
        workload: &WorkloadAnalysis,
        hint: Option<&str>,
        previous: &[TuningIteration],
    ) -> Result<Vec<Recommendation>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let tools_array = tool_schema();
        let function_decls: Vec<serde_json::Value> = tools_array
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("tool_schema did not return an array"))?
            .iter()
            .map(|t| {
                json!({
                    "name": t["name"],
                    "description": t["description"],
                    "parameters": t["input_schema"]
                })
            })
            .collect();

        let body = json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": format!("{}\n\n{}", build_system_prompt(), build_user_message(context, workload, hint, previous)) }]
            }],
            "tools": [{ "functionDeclarations": function_decls }],
            "toolConfig": {
                "functionCallingConfig": { "mode": "ANY" }
            }
        });

        let resp = client
            .post(format!(
                "{}/v1beta/models/{}:generateContent",
                self.base_url, self.model
            ))
            .header("x-goog-api-key", &self.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Gemini API error {status}: {text}");
        }

        let data: serde_json::Value = resp.json().await?;
        let mut recs = Vec::new();

        if let Some(parts) = data["candidates"][0]["content"]["parts"].as_array() {
            for part in parts {
                if let Some(fc) = part.get("functionCall") {
                    if let Some(name) = fc["name"].as_str() {
                        if let Some(rec) = parse_tool_call(name, &fc["args"]) {
                            recs.push(rec);
                        }
                    }
                }
            }
        }

        Ok(recs)
    }

    fn name(&self) -> &str {
        "Gemini"
    }
}

// ---------------------------------------------------------------------------
// Bedrock Advisor (via AWS CLI)
// ---------------------------------------------------------------------------

struct BedrockAdvisor {
    model: String,
    region: Option<String>,
}

#[async_trait::async_trait]
impl TuningAdvisor for BedrockAdvisor {
    async fn recommend(
        &self,
        context: &PgContext,
        workload: &WorkloadAnalysis,
        hint: Option<&str>,
        previous: &[TuningIteration],
    ) -> Result<Vec<Recommendation>> {
        let tools_array = tool_schema();
        let tool_specs: Vec<serde_json::Value> = tools_array
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("tool_schema did not return an array"))?
            .iter()
            .map(|t| {
                json!({
                    "toolSpec": {
                        "name": t["name"],
                        "description": t["description"],
                        "inputSchema": { "json": t["input_schema"] }
                    }
                })
            })
            .collect();

        let tool_config = json!({
            "tools": tool_specs
        });

        let messages = json!([{
            "role": "user",
            "content": [{
                "text": format!("{}\n\n{}", build_system_prompt(), build_user_message(context, workload, hint, previous))
            }]
        }]);

        let mut cmd = tokio::process::Command::new("aws");
        cmd.arg("bedrock-runtime")
            .arg("converse")
            .arg("--model-id")
            .arg(&self.model)
            .arg("--messages")
            .arg(messages.to_string())
            .arg("--tool-config")
            .arg(tool_config.to_string())
            .arg("--output")
            .arg("json");

        if let Some(ref region) = self.region {
            cmd.arg("--region").arg(region);
        }

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("AWS Bedrock CLI error: {stderr}");
        }

        let resp: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let mut recs = Vec::new();

        if let Some(content) = resp["output"]["message"]["content"].as_array() {
            for block in content {
                if let Some(tool_use) = block.get("toolUse") {
                    if let Some(name) = tool_use["name"].as_str() {
                        if let Some(rec) = parse_tool_call(name, &tool_use["input"]) {
                            recs.push(rec);
                        }
                    }
                }
            }
        }

        Ok(recs)
    }

    fn name(&self) -> &str {
        "Bedrock"
    }
}

// ---------------------------------------------------------------------------
// Ollama Advisor
// ---------------------------------------------------------------------------

struct OllamaAdvisor {
    model: String,
    base_url: String,
}

#[async_trait::async_trait]
impl TuningAdvisor for OllamaAdvisor {
    async fn recommend(
        &self,
        context: &PgContext,
        workload: &WorkloadAnalysis,
        hint: Option<&str>,
        previous: &[TuningIteration],
    ) -> Result<Vec<Recommendation>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let prompt = format!(
            "{}\n\n{}\n\nRespond with a JSON array of recommendation objects. \
             Each object must have a \"type\" field (config_change, create_index, \
             query_rewrite, or schema_change) and the corresponding fields.",
            build_system_prompt(),
            build_user_message(context, workload, hint, previous),
        );

        let body = json!({
            "model": self.model,
            "prompt": prompt,
            "format": "json",
            "stream": false,
        });

        let resp = client
            .post(format!("{}/api/generate", self.base_url))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Ollama API error {status}: {text}");
        }

        let data: serde_json::Value = resp.json().await?;
        let response_text = data["response"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Ollama response missing 'response' field"))?;

        let recs: Vec<Recommendation> = serde_json::from_str(response_text)
            .map_err(|e| anyhow::anyhow!("Failed to parse Ollama recommendations: {e}"))?;

        Ok(recs)
    }

    fn name(&self) -> &str {
        "Ollama"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_calls() {
        let input = json!({
            "parameter": "shared_buffers",
            "current_value": "128MB",
            "recommended_value": "1GB",
            "rationale": "More memory for caching"
        });
        let rec = parse_tool_call("config_change", &input).unwrap();
        assert!(matches!(rec, Recommendation::ConfigChange { .. }));

        let input = json!({
            "table": "orders",
            "columns": ["status", "created_at"],
            "sql": "CREATE INDEX idx_orders_status ON orders (status, created_at)",
            "rationale": "Speed up status queries"
        });
        let rec = parse_tool_call("create_index", &input).unwrap();
        assert!(matches!(rec, Recommendation::CreateIndex { .. }));

        assert!(parse_tool_call("unknown_tool", &json!({})).is_none());
    }

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("PostgreSQL tuning expert"));
        assert!(prompt.contains("config_change"));
        assert!(prompt.contains("create_index"));
        assert!(prompt.contains("query_rewrite"));
        assert!(prompt.contains("schema_change"));
    }
}

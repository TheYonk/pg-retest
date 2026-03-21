use anyhow::Result;
use serde_json::json;

use super::analyze::WorkloadAnalysis;
use super::plan::TransformPlan;

/// Trait for LLM-backed plan generation.
#[async_trait::async_trait]
pub trait LlmPlanner: Send + Sync {
    async fn generate_plan(
        &self,
        analysis: &WorkloadAnalysis,
        prompt: &str,
    ) -> Result<TransformPlan>;

    fn name(&self) -> &str;
}

/// Supported LLM providers.
#[derive(Debug, Clone, Copy)]
pub enum LlmProvider {
    Claude,
    OpenAi,
    Gemini,
    Bedrock,
    Ollama,
}

impl std::str::FromStr for LlmProvider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Ok(Self::Claude),
            "openai" | "gpt" => Ok(Self::OpenAi),
            "gemini" | "google" => Ok(Self::Gemini),
            "bedrock" | "aws" => Ok(Self::Bedrock),
            "ollama" | "local" => Ok(Self::Ollama),
            other => {
                anyhow::bail!("Unknown LLM provider: {other}. Supported: claude, openai, gemini, bedrock, ollama")
            }
        }
    }
}

/// Configuration for creating an LLM planner.
pub struct PlannerConfig {
    pub provider: LlmProvider,
    pub api_key: String,
    pub api_url: Option<String>,
    pub model: Option<String>,
}

/// Create an LLM planner from config.
pub fn create_planner(config: PlannerConfig) -> Box<dyn LlmPlanner> {
    match config.provider {
        LlmProvider::Claude => Box::new(ClaudePlanner {
            api_key: config.api_key,
            api_url: config
                .api_url
                .unwrap_or_else(|| "https://api.anthropic.com".into()),
            model: config
                .model
                .unwrap_or_else(|| "claude-sonnet-4-20250514".into()),
        }),
        LlmProvider::OpenAi => Box::new(OpenAiPlanner {
            api_key: config.api_key,
            api_url: config
                .api_url
                .unwrap_or_else(|| "https://api.openai.com".into()),
            model: config.model.unwrap_or_else(|| "gpt-4o".into()),
        }),
        LlmProvider::Gemini => Box::new(GeminiPlanner {
            api_key: config.api_key,
            api_url: config
                .api_url
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into()),
            model: config.model.unwrap_or_else(|| "gemini-2.5-flash".into()),
        }),
        LlmProvider::Bedrock => Box::new(BedrockPlanner {
            model: config
                .model
                .unwrap_or_else(|| "us.anthropic.claude-sonnet-4-20250514-v1:0".into()),
            region: config.api_url, // reuse api_url as AWS region override
        }),
        LlmProvider::Ollama => Box::new(OllamaPlanner {
            api_url: config
                .api_url
                .unwrap_or_else(|| "http://localhost:11434".into()),
            model: config.model.unwrap_or_else(|| "llama3".into()),
        }),
    }
}

// -- Prompt building (public for testing and dry-run) -------------------------

pub fn build_system_prompt() -> String {
    r#"You are a PostgreSQL workload planning assistant. Given a captured workload analysis and a user's scenario description, generate a transform plan that modifies the workload to simulate the described scenario.

Rules:
- Map the user's intent to the identified query groups
- Assign scaling factors to groups that should change
- Generate SQL for any new queries the scenario requires
- Use parameter patterns from the analysis to generate realistic SQL
- Preserve groups not mentioned in the scenario at scale 1.0
- Provide human-readable descriptions for each group and transform
- query_indices must reference valid indices from the analysis"#
        .into()
}

pub fn build_user_message(analysis: &WorkloadAnalysis, user_prompt: &str) -> String {
    let analysis_json = serde_json::to_string_pretty(analysis).unwrap_or_default();
    format!(
        "## Workload Analysis\n```json\n{analysis_json}\n```\n\n## User Scenario\n{user_prompt}"
    )
}

fn tool_schema() -> serde_json::Value {
    json!({
        "name": "generate_transform_plan",
        "description": "Generate a workload transform plan based on the analysis and user scenario",
        "input_schema": {
            "type": "object",
            "required": ["version", "source", "analysis", "groups", "transforms"],
            "properties": {
                "version": { "type": "integer" },
                "source": {
                    "type": "object",
                    "properties": {
                        "profile": { "type": "string" },
                        "prompt": { "type": "string" }
                    },
                    "required": ["profile", "prompt"]
                },
                "analysis": {
                    "type": "object",
                    "properties": {
                        "total_queries": { "type": "integer" },
                        "total_sessions": { "type": "integer" },
                        "groups_identified": { "type": "integer" }
                    },
                    "required": ["total_queries", "total_sessions", "groups_identified"]
                },
                "groups": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "tables": { "type": "array", "items": { "type": "string" } },
                            "query_indices": { "type": "array", "items": { "type": "integer" } },
                            "session_ids": { "type": "array", "items": { "type": "integer" } },
                            "query_count": { "type": "integer" }
                        },
                        "required": ["name", "description", "tables", "query_indices", "session_ids", "query_count"]
                    }
                },
                "transforms": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string", "enum": ["scale", "inject", "inject_session", "remove"] },
                            "group": { "type": "string" },
                            "factor": { "type": "number" },
                            "stagger_ms": { "type": "integer" },
                            "description": { "type": "string" },
                            "sql": { "type": "string" },
                            "after_group": { "type": "string" },
                            "frequency": { "type": "number" },
                            "estimated_duration_us": { "type": "integer" },
                            "queries": { "type": "array" },
                            "repeat": { "type": "integer" },
                            "interval_ms": { "type": "integer" }
                        },
                        "required": ["type"]
                    }
                }
            }
        }
    })
}

// -- Claude Provider ----------------------------------------------------------

struct ClaudePlanner {
    api_key: String,
    api_url: String,
    model: String,
}

#[async_trait::async_trait]
impl LlmPlanner for ClaudePlanner {
    async fn generate_plan(
        &self,
        analysis: &WorkloadAnalysis,
        prompt: &str,
    ) -> Result<TransformPlan> {
        let client = reqwest::Client::new();
        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": build_system_prompt(),
            "tools": [tool_schema()],
            "tool_choice": { "type": "tool", "name": "generate_transform_plan" },
            "messages": [{ "role": "user", "content": build_user_message(analysis, prompt) }]
        });

        let resp = client
            .post(format!("{}/v1/messages", self.api_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await?;

        let status = resp.status();
        let resp_json: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let err_msg = resp_json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            anyhow::bail!("Claude API error ({status}): {err_msg}");
        }

        let content = resp_json["content"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No content in response"))?;

        for block in content {
            if block["type"] == "tool_use" {
                let input = &block["input"];
                let plan: TransformPlan = serde_json::from_value(input.clone())?;
                return Ok(plan);
            }
        }

        anyhow::bail!("No tool_use block in Claude response")
    }

    fn name(&self) -> &str {
        "claude"
    }
}

// -- OpenAI Provider ----------------------------------------------------------

struct OpenAiPlanner {
    api_key: String,
    api_url: String,
    model: String,
}

#[async_trait::async_trait]
impl LlmPlanner for OpenAiPlanner {
    async fn generate_plan(
        &self,
        analysis: &WorkloadAnalysis,
        prompt: &str,
    ) -> Result<TransformPlan> {
        let client = reqwest::Client::new();
        let function_def = tool_schema();

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
                { "role": "user", "content": build_user_message(analysis, prompt) }
            ],
            "tools": [{ "type": "function", "function": function_def }],
            "tool_choice": { "type": "function", "function": { "name": "generate_transform_plan" } }
        });
        body[token_key] = json!(4096);

        let resp = client
            .post(format!("{}/v1/chat/completions", self.api_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await?;

        let status = resp.status();
        let resp_json: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let err_msg = resp_json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            anyhow::bail!("OpenAI API error ({status}): {err_msg}");
        }

        let tool_call =
            &resp_json["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"];
        let args_str = tool_call
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No tool call arguments"))?;
        let plan: TransformPlan = serde_json::from_str(args_str)?;
        Ok(plan)
    }

    fn name(&self) -> &str {
        "openai"
    }
}

// -- Gemini Provider ----------------------------------------------------------

struct GeminiPlanner {
    api_key: String,
    api_url: String,
    model: String,
}

#[async_trait::async_trait]
impl LlmPlanner for GeminiPlanner {
    async fn generate_plan(
        &self,
        analysis: &WorkloadAnalysis,
        prompt: &str,
    ) -> Result<TransformPlan> {
        let client = reqwest::Client::new();
        let schema = tool_schema();

        let body = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [{ "text": format!("{}\n\n{}", build_system_prompt(), build_user_message(analysis, prompt)) }]
                }
            ],
            "tools": [{
                "functionDeclarations": [{
                    "name": schema["name"],
                    "description": schema["description"],
                    "parameters": schema["input_schema"]
                }]
            }],
            "toolConfig": {
                "functionCallingConfig": { "mode": "ANY" }
            }
        });

        let resp = client
            .post(format!(
                "{}/v1beta/models/{}:generateContent",
                self.api_url, self.model
            ))
            .header("x-goog-api-key", &self.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await?;

        let status = resp.status();
        let resp_json: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let err_msg = resp_json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            anyhow::bail!("Gemini API error ({status}): {err_msg}");
        }

        // Gemini returns functionCall in candidates[0].content.parts[]
        let parts = resp_json["candidates"][0]["content"]["parts"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No parts in Gemini response"))?;

        for part in parts {
            if let Some(fc) = part.get("functionCall") {
                let args = &fc["args"];
                let plan: TransformPlan = serde_json::from_value(args.clone())?;
                return Ok(plan);
            }
        }

        anyhow::bail!("No functionCall in Gemini response")
    }

    fn name(&self) -> &str {
        "gemini"
    }
}

// -- Bedrock Provider (via AWS CLI) -------------------------------------------

struct BedrockPlanner {
    model: String,
    region: Option<String>,
}

#[async_trait::async_trait]
impl LlmPlanner for BedrockPlanner {
    async fn generate_plan(
        &self,
        analysis: &WorkloadAnalysis,
        prompt: &str,
    ) -> Result<TransformPlan> {
        let schema = tool_schema();
        let tool_config = json!({
            "tools": [{
                "toolSpec": {
                    "name": schema["name"],
                    "description": schema["description"],
                    "inputSchema": { "json": schema["input_schema"] }
                }
            }],
            "toolChoice": { "any": {} }
        });

        let messages = json!([{
            "role": "user",
            "content": [{
                "text": format!("{}\n\n{}", build_system_prompt(), build_user_message(analysis, prompt))
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
            anyhow::bail!("AWS Bedrock CLI error: {stderr}");
        }

        let resp: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        // Bedrock Converse returns tool use in output.content[]
        let content = resp["output"]["message"]["content"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No content in Bedrock response"))?;

        for block in content {
            if let Some(tool_use) = block.get("toolUse") {
                let input = &tool_use["input"];
                let plan: TransformPlan = serde_json::from_value(input.clone())?;
                return Ok(plan);
            }
        }

        anyhow::bail!("No toolUse in Bedrock response")
    }

    fn name(&self) -> &str {
        "bedrock"
    }
}

// -- Ollama Provider ----------------------------------------------------------

struct OllamaPlanner {
    api_url: String,
    model: String,
}

#[async_trait::async_trait]
impl LlmPlanner for OllamaPlanner {
    async fn generate_plan(
        &self,
        analysis: &WorkloadAnalysis,
        prompt: &str,
    ) -> Result<TransformPlan> {
        let client = reqwest::Client::new();
        let body = json!({
            "model": self.model,
            "stream": false,
            "format": "json",
            "system": build_system_prompt(),
            "prompt": build_user_message(analysis, prompt),
        });

        let resp = client
            .post(format!("{}/api/generate", self.api_url))
            .json(&body)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await?;

        let resp_json: serde_json::Value = resp.json().await?;
        let response_text = resp_json["response"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No response from Ollama"))?;

        let plan: TransformPlan = serde_json::from_str(response_text)?;
        Ok(plan)
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::analyze::{ProfileSummary, WorkloadAnalysis};

    #[test]
    fn test_build_prompts() {
        let analysis = WorkloadAnalysis {
            profile_summary: ProfileSummary {
                total_queries: 100,
                total_sessions: 5,
                capture_duration_s: 60.0,
                source_host: "localhost".into(),
            },
            query_groups: vec![],
            ungrouped_queries: 10,
        };

        let prompt = build_system_prompt();
        assert!(prompt.contains("workload planning"));

        let user_msg = build_user_message(&analysis, "Scale product queries 5x");
        assert!(user_msg.contains("100"));
        assert!(user_msg.contains("Scale product queries 5x"));
    }

    #[test]
    fn test_parse_plan_from_tool_call_json() {
        let tool_json = serde_json::json!({
            "version": 1,
            "source": { "profile": "test.wkl", "prompt": "test" },
            "analysis": { "total_queries": 10, "total_sessions": 2, "groups_identified": 1 },
            "groups": [{
                "name": "products",
                "description": "Product queries",
                "tables": ["products"],
                "query_indices": [0, 1],
                "session_ids": [1],
                "query_count": 2
            }],
            "transforms": [{
                "type": "scale",
                "group": "products",
                "factor": 3.0,
                "stagger_ms": 10
            }]
        });

        let plan: TransformPlan = serde_json::from_value(tool_json).unwrap();
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.transforms.len(), 1);
    }

    #[test]
    fn test_provider_from_str() {
        assert!(matches!(
            "claude".parse::<LlmProvider>().unwrap(),
            LlmProvider::Claude
        ));
        assert!(matches!(
            "openai".parse::<LlmProvider>().unwrap(),
            LlmProvider::OpenAi
        ));
        assert!(matches!(
            "gemini".parse::<LlmProvider>().unwrap(),
            LlmProvider::Gemini
        ));
        assert!(matches!(
            "bedrock".parse::<LlmProvider>().unwrap(),
            LlmProvider::Bedrock
        ));
        assert!(matches!(
            "aws".parse::<LlmProvider>().unwrap(),
            LlmProvider::Bedrock
        ));
        assert!(matches!(
            "ollama".parse::<LlmProvider>().unwrap(),
            LlmProvider::Ollama
        ));
        assert!("invalid".parse::<LlmProvider>().is_err());
    }

    #[test]
    fn test_create_planner() {
        let planner = create_planner(PlannerConfig {
            provider: LlmProvider::Claude,
            api_key: "test-key".into(),
            api_url: None,
            model: None,
        });
        assert_eq!(planner.name(), "claude");

        let planner = create_planner(PlannerConfig {
            provider: LlmProvider::OpenAi,
            api_key: "test-key".into(),
            api_url: None,
            model: None,
        });
        assert_eq!(planner.name(), "openai");

        let planner = create_planner(PlannerConfig {
            provider: LlmProvider::Gemini,
            api_key: "test-key".into(),
            api_url: None,
            model: None,
        });
        assert_eq!(planner.name(), "gemini");

        let planner = create_planner(PlannerConfig {
            provider: LlmProvider::Bedrock,
            api_key: String::new(),
            api_url: None,
            model: None,
        });
        assert_eq!(planner.name(), "bedrock");

        let planner = create_planner(PlannerConfig {
            provider: LlmProvider::Ollama,
            api_key: String::new(),
            api_url: None,
            model: None,
        });
        assert_eq!(planner.name(), "ollama");
    }
}

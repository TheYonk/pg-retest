pub mod analyze;
pub mod engine;
pub mod mysql_to_pg;
pub mod plan;

/// Result of transforming a single SQL statement.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformResult {
    /// SQL was transformed to PG-compatible syntax.
    Transformed(String),
    /// SQL could not be transformed and should be skipped.
    Skipped { reason: String },
    /// SQL is already PG-compatible (no changes needed).
    Unchanged,
}

/// A single SQL transformation rule.
pub trait SqlTransformer: Send + Sync {
    fn transform(&self, sql: &str) -> TransformResult;
    fn name(&self) -> &str;
}

/// A composable pipeline of SQL transformers.
pub struct TransformPipeline {
    transformers: Vec<Box<dyn SqlTransformer>>,
}

impl TransformPipeline {
    pub fn new(transformers: Vec<Box<dyn SqlTransformer>>) -> Self {
        Self { transformers }
    }

    pub fn apply(&self, sql: &str) -> TransformResult {
        let mut current = sql.to_string();
        let mut was_transformed = false;

        for transformer in &self.transformers {
            match transformer.transform(&current) {
                TransformResult::Transformed(new_sql) => {
                    current = new_sql;
                    was_transformed = true;
                }
                TransformResult::Skipped { reason } => {
                    return TransformResult::Skipped { reason };
                }
                TransformResult::Unchanged => {}
            }
        }

        if was_transformed {
            TransformResult::Transformed(current)
        } else {
            TransformResult::Unchanged
        }
    }
}

/// Summary of transform results across a workload.
#[derive(Debug, Default)]
pub struct TransformReport {
    pub total_queries: usize,
    pub transformed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub skip_reasons: Vec<(String, String)>,
}

impl TransformReport {
    pub fn record(&mut self, sql: &str, result: &TransformResult) {
        self.total_queries += 1;
        match result {
            TransformResult::Transformed(_) => self.transformed += 1,
            TransformResult::Unchanged => self.unchanged += 1,
            TransformResult::Skipped { reason } => {
                self.skipped += 1;
                let preview: String = sql.chars().take(80).collect();
                self.skip_reasons.push((preview, reason.clone()));
            }
        }
    }

    pub fn print_summary(&self) {
        println!();
        println!("  Transform Report");
        println!("  ================");
        println!("  Total queries:  {}", self.total_queries);
        println!("  Transformed:    {}", self.transformed);
        println!("  Unchanged:      {}", self.unchanged);
        println!("  Skipped:        {}", self.skipped);
        if !self.skip_reasons.is_empty() {
            println!();
            println!("  Skipped queries:");
            for (sql, reason) in self.skip_reasons.iter().take(10) {
                println!("    - {sql}");
                println!("      Reason: {reason}");
            }
            if self.skip_reasons.len() > 10 {
                println!("    ... and {} more", self.skip_reasons.len() - 10);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UppercaseTransformer;
    impl SqlTransformer for UppercaseTransformer {
        fn transform(&self, sql: &str) -> TransformResult {
            let upper = sql.to_uppercase();
            if upper == sql {
                TransformResult::Unchanged
            } else {
                TransformResult::Transformed(upper)
            }
        }
        fn name(&self) -> &str {
            "uppercase"
        }
    }

    struct SkipDdlTransformer;
    impl SqlTransformer for SkipDdlTransformer {
        fn transform(&self, sql: &str) -> TransformResult {
            if sql.to_uppercase().starts_with("CREATE") {
                TransformResult::Skipped {
                    reason: "DDL not supported".into(),
                }
            } else {
                TransformResult::Unchanged
            }
        }
        fn name(&self) -> &str {
            "skip_ddl"
        }
    }

    #[test]
    fn test_pipeline_transforms_in_order() {
        let pipeline = TransformPipeline::new(vec![Box::new(UppercaseTransformer)]);
        let result = pipeline.apply("select 1");
        assert_eq!(result, TransformResult::Transformed("SELECT 1".into()));
    }

    #[test]
    fn test_pipeline_unchanged_passthrough() {
        let pipeline = TransformPipeline::new(vec![Box::new(UppercaseTransformer)]);
        let result = pipeline.apply("SELECT 1");
        assert_eq!(result, TransformResult::Unchanged);
    }

    #[test]
    fn test_pipeline_skip_short_circuits() {
        let pipeline = TransformPipeline::new(vec![
            Box::new(SkipDdlTransformer),
            Box::new(UppercaseTransformer),
        ]);
        let result = pipeline.apply("CREATE TABLE foo (id int)");
        assert!(matches!(result, TransformResult::Skipped { .. }));
    }

    #[test]
    fn test_transform_report() {
        let mut report = TransformReport::default();
        report.record("SELECT 1", &TransformResult::Unchanged);
        report.record("select 1", &TransformResult::Transformed("SELECT 1".into()));
        report.record(
            "CREATE TABLE x",
            &TransformResult::Skipped {
                reason: "DDL".into(),
            },
        );
        assert_eq!(report.total_queries, 3);
        assert_eq!(report.transformed, 1);
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.skipped, 1);
    }
}

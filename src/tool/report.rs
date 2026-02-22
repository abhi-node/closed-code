use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;

use super::{ParamBuilder, Tool};

/// Special tool for sub-agents to structure their final output.
///
/// When the sub-agent's tool-call loop detects a call to "create_report",
/// it extracts the arguments as the agent's response and terminates the loop.
/// The execute() method is a fallback that should not normally be reached.
#[derive(Debug)]
pub struct CreateReportTool;

impl CreateReportTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CreateReportTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CreateReportTool {
    fn name(&self) -> &str {
        "create_report"
    }

    fn description(&self) -> &str {
        "Submit your research findings as a structured report. Call this when you have \
         gathered enough information to answer the task. This is REQUIRED — you must \
         call this tool to deliver your results."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "summary",
                    "A brief 1-3 sentence summary of your findings.",
                    true,
                )
                .string(
                    "detailed_report",
                    "The full detailed report with your findings, analysis, \
                     and recommendations. Use markdown formatting. Be thorough.",
                    true,
                )
                .string(
                    "code_snippets",
                    "Optional JSON array of code snippets: \
                     [{\"name\": \"file.rs\", \"language\": \"rust\", \
                     \"content\": \"...\"}]. Include relevant code you found \
                     or propose.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, _args: Value) -> Result<Value> {
        // This should never be called directly — the sub-agent loop
        // intercepts create_report calls before reaching execute().
        // If somehow reached, return a success response.
        Ok(json!({
            "status": "report_received",
            "note": "Report was processed by the sub-agent framework."
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Explore, Mode::Plan, Mode::Execute]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_report_tool_properties() {
        let tool = CreateReportTool::new();
        assert_eq!(tool.name(), "create_report");
        assert!(tool.description().contains("structured report"));
    }

    #[test]
    fn create_report_declaration_has_required_params() {
        let tool = CreateReportTool::new();
        let decl = tool.declaration();
        assert_eq!(decl.name, "create_report");
        let required = decl.parameters.required.as_ref().unwrap();
        assert!(required.contains(&"summary".to_string()));
        assert!(required.contains(&"detailed_report".to_string()));
    }

    #[tokio::test]
    async fn create_report_execute_fallback() {
        let tool = CreateReportTool::new();
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["status"], "report_received");
    }
}

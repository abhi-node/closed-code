use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

use crate::error::Result;
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;

use super::{ParamBuilder, Tool};

/// Tool that lets the LLM retrieve the current accepted plan.
#[derive(Debug)]
pub struct GetPlanTool {
    plan: Arc<RwLock<Option<String>>>,
}

impl GetPlanTool {
    pub fn new(plan: Arc<RwLock<Option<String>>>) -> Self {
        Self { plan }
    }
}

#[async_trait]
impl Tool for GetPlanTool {
    fn name(&self) -> &str {
        "get_plan"
    }

    fn description(&self) -> &str {
        "Retrieve the current accepted implementation plan. Use this to review the plan \
         before starting work or to check what steps remain."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new().build(),
        }
    }

    async fn execute(&self, _args: Value) -> Result<Value> {
        let plan = self.plan.read().map_err(|e| {
            crate::error::ClosedCodeError::ToolError {
                name: "get_plan".into(),
                message: format!("Failed to read plan: {}", e),
            }
        })?;
        match plan.as_deref() {
            Some(text) => Ok(json!({ "plan": text })),
            None => Ok(json!({ "plan": null, "message": "No plan is currently set." })),
        }
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![
            Mode::Explore,
            Mode::Plan,
            Mode::Guided,
            Mode::Execute,
            Mode::Auto,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_plan_no_plan() {
        let plan = Arc::new(RwLock::new(None));
        let tool = GetPlanTool::new(plan);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result["plan"].is_null());
        assert!(result["message"].as_str().unwrap().contains("No plan"));
    }

    #[tokio::test]
    async fn get_plan_with_plan() {
        let plan = Arc::new(RwLock::new(Some("Step 1: Do X\nStep 2: Do Y".to_string())));
        let tool = GetPlanTool::new(plan);
        let result = tool.execute(json!({})).await.unwrap();
        assert_eq!(result["plan"], "Step 1: Do X\nStep 2: Do Y");
    }

    #[test]
    fn get_plan_available_all_modes() {
        let plan = Arc::new(RwLock::new(None));
        let tool = GetPlanTool::new(plan);
        let modes = tool.available_modes();
        assert_eq!(modes.len(), 5);
    }

    #[test]
    fn get_plan_declaration() {
        let plan = Arc::new(RwLock::new(None));
        let tool = GetPlanTool::new(plan);
        let decl = tool.declaration();
        assert_eq!(decl.name, "get_plan");
    }
}

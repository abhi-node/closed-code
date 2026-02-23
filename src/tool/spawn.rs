use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::agent::explorer::ExplorerAgent;
use crate::agent::message::{AgentRequest, ToolProgressFn};
use crate::agent::orchestrator::OrchestratorEvent;
use crate::agent::planner::PlannerAgent;
use crate::agent::web_searcher::WebSearchAgent;
use crate::agent::Agent;
use crate::error::Result;
use crate::gemini::types::FunctionDeclaration;
use crate::gemini::GeminiClient;
use crate::mode::Mode;
use crate::sandbox::Sandbox;

use super::{ParamBuilder, Tool};

/// Create a progress callback from an event sender and agent type.
/// Public so the orchestrator can create callbacks for directly-invoked agents
/// (CommitAgent, ReviewAgent).
pub fn make_agent_progress_callback(
    tx: UnboundedSender<OrchestratorEvent>,
    agent_type: &str,
) -> ToolProgressFn {
    let agent_type = agent_type.to_string();
    Arc::new(move |tool_name: &str, args_display: &str| {
        let _ = tx.send(OrchestratorEvent::AgentToolUpdate {
            agent_type: agent_type.clone(),
            tool_name: tool_name.to_string(),
            args_display: args_display.to_string(),
        });
    })
}

// ── SpawnExplorerTool ──

pub struct SpawnExplorerTool {
    client: Arc<GeminiClient>,
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
    event_tx: Option<UnboundedSender<OrchestratorEvent>>,
}

impl std::fmt::Debug for SpawnExplorerTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnExplorerTool")
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

impl SpawnExplorerTool {
    pub fn new(
        client: Arc<GeminiClient>,
        working_directory: PathBuf,
        sandbox: Arc<dyn Sandbox>,
    ) -> Self {
        Self {
            client,
            working_directory,
            sandbox,
            event_tx: None,
        }
    }

    pub fn with_event_tx(mut self, tx: Option<UnboundedSender<OrchestratorEvent>>) -> Self {
        self.event_tx = tx;
        self
    }
}

#[async_trait]
impl Tool for SpawnExplorerTool {
    fn name(&self) -> &str {
        "spawn_explorer"
    }

    fn description(&self) -> &str {
        "Spawn an explorer sub-agent to research the codebase. The explorer will \
         autonomously read files, search for patterns, and produce a structured \
         report. Use this when you need to understand code architecture, find \
         implementations, or analyze patterns before answering the user."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "task",
                    "A clear description of what the explorer should research. \
                     Be specific: name files, patterns, or questions.",
                    true,
                )
                .string(
                    "context",
                    "Optional context from the current conversation that would \
                     help the explorer understand what is needed.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let task = args["task"]
            .as_str()
            .unwrap_or("Explore the codebase")
            .to_string();
        let context_str = args["context"].as_str().unwrap_or("");

        let mut request =
            AgentRequest::new(task, self.working_directory.to_string_lossy().to_string());
        if !context_str.is_empty() {
            request = request.with_context(vec![context_str.to_string()]);
        }

        tracing::info!("Spawning explorer agent: {}", request.task);

        let mut agent = ExplorerAgent::new(self.working_directory.clone(), self.sandbox.clone());
        if let Some(ref tx) = self.event_tx {
            agent = agent.with_progress(make_agent_progress_callback(tx.clone(), "explorer"));
        }
        let response = agent.run(&self.client, request).await?;

        Ok(json!({
            "agent_type": response.agent_type,
            "summary": response.summary,
            "detailed_report": response.detailed_report,
            "artifact_count": response.artifacts.len(),
            "artifacts": response.artifacts.iter().map(|a| {
                json!({
                    "name": a.name,
                    "content": a.content,
                })
            }).collect::<Vec<_>>(),
        }))
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

// ── SpawnPlannerTool ──

pub struct SpawnPlannerTool {
    client: Arc<GeminiClient>,
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
    event_tx: Option<UnboundedSender<OrchestratorEvent>>,
}

impl std::fmt::Debug for SpawnPlannerTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnPlannerTool")
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

impl SpawnPlannerTool {
    pub fn new(
        client: Arc<GeminiClient>,
        working_directory: PathBuf,
        sandbox: Arc<dyn Sandbox>,
    ) -> Self {
        Self {
            client,
            working_directory,
            sandbox,
            event_tx: None,
        }
    }

    pub fn with_event_tx(mut self, tx: Option<UnboundedSender<OrchestratorEvent>>) -> Self {
        self.event_tx = tx;
        self
    }
}

#[async_trait]
impl Tool for SpawnPlannerTool {
    fn name(&self) -> &str {
        "spawn_planner"
    }

    fn description(&self) -> &str {
        "Spawn a planner sub-agent to create a structured implementation plan. \
         The planner analyzes the codebase and produces step-by-step plans with \
         affected files, patterns to follow, and risk assessments."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "task",
                    "A clear description of what needs to be planned. \
                     Include goals, constraints, and any known requirements.",
                    true,
                )
                .string(
                    "context",
                    "Optional context from the current conversation.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let task = args["task"]
            .as_str()
            .unwrap_or("Create an implementation plan")
            .to_string();
        let context_str = args["context"].as_str().unwrap_or("");

        let mut request =
            AgentRequest::new(task, self.working_directory.to_string_lossy().to_string());
        if !context_str.is_empty() {
            request = request.with_context(vec![context_str.to_string()]);
        }

        tracing::info!("Spawning planner agent: {}", request.task);

        let mut agent = PlannerAgent::new(self.working_directory.clone(), self.sandbox.clone());
        if let Some(ref tx) = self.event_tx {
            agent = agent.with_progress(make_agent_progress_callback(tx.clone(), "planner"));
        }
        let response = agent.run(&self.client, request).await?;

        Ok(json!({
            "agent_type": response.agent_type,
            "summary": response.summary,
            "detailed_report": response.detailed_report,
            "artifact_count": response.artifacts.len(),
            "artifacts": response.artifacts.iter().map(|a| {
                json!({
                    "name": a.name,
                    "content": a.content,
                })
            }).collect::<Vec<_>>(),
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Plan, Mode::Guided, Mode::Execute, Mode::Auto]
    }
}

// ── SpawnWebSearchTool ──

#[derive(Debug)]
pub struct SpawnWebSearchTool {
    client: Arc<GeminiClient>,
    #[allow(dead_code)]
    working_directory: PathBuf,
}

impl SpawnWebSearchTool {
    pub fn new(client: Arc<GeminiClient>, working_directory: PathBuf) -> Self {
        Self {
            client,
            working_directory,
        }
    }
}

#[async_trait]
impl Tool for SpawnWebSearchTool {
    fn name(&self) -> &str {
        "spawn_web_search"
    }

    fn description(&self) -> &str {
        "Spawn a web search sub-agent to research a topic online. \
         Uses Google Search grounding to find recent documentation, \
         best practices, and solutions. Returns findings with sources."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "query",
                    "The research query. Be specific and include relevant \
                     technology names and version numbers.",
                    true,
                )
                .string(
                    "context",
                    "Optional context about why this search is needed.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let query = args["query"]
            .as_str()
            .unwrap_or("Search the web")
            .to_string();
        let context_str = args["context"].as_str().unwrap_or("");

        let mut request =
            AgentRequest::new(query, self.working_directory.to_string_lossy().to_string());
        if !context_str.is_empty() {
            request = request.with_context(vec![context_str.to_string()]);
        }

        tracing::info!("Spawning web search agent: {}", request.task);

        let agent = WebSearchAgent::new();
        let response = agent.run(&self.client, request).await?;

        Ok(json!({
            "agent_type": response.agent_type,
            "summary": response.summary,
            "detailed_report": response.detailed_report,
            "artifact_count": response.artifacts.len(),
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Plan]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::mock::MockSandbox;

    fn mock_sandbox() -> Arc<dyn Sandbox> {
        Arc::new(MockSandbox::new(PathBuf::from("/tmp")))
    }

    #[test]
    fn spawn_explorer_tool_properties() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let tool = SpawnExplorerTool::new(client, PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(tool.name(), "spawn_explorer");
        assert!(tool.available_modes().contains(&Mode::Explore));
        assert!(tool.available_modes().contains(&Mode::Plan));
        assert!(tool.available_modes().contains(&Mode::Execute));
        assert!(tool.available_modes().contains(&Mode::Auto));
    }

    #[test]
    fn spawn_planner_tool_properties() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let tool = SpawnPlannerTool::new(client, PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(tool.name(), "spawn_planner");
        assert!(tool.available_modes().contains(&Mode::Plan));
        assert!(tool.available_modes().contains(&Mode::Guided));
        assert!(tool.available_modes().contains(&Mode::Execute));
        assert!(tool.available_modes().contains(&Mode::Auto));
        assert!(!tool.available_modes().contains(&Mode::Explore));
    }

    #[test]
    fn spawn_web_search_tool_plan_mode_only() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let tool = SpawnWebSearchTool::new(client, PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "spawn_web_search");
        assert_eq!(tool.available_modes(), vec![Mode::Plan]);
    }
}

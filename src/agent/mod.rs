pub mod cache;
pub mod commit_agent;
pub mod explorer;
pub mod message;
pub mod orchestrator;
pub mod planner;
pub mod review_agent;
pub mod web_searcher;

use async_trait::async_trait;
use std::fmt::Debug;

use crate::error::Result;
use crate::gemini::GeminiClient;

pub use message::{AgentRequest, AgentResponse};

/// A sub-agent that can independently research and report back.
///
/// Key differences from the Tool trait:
/// - Agents own their own ToolRegistry and run their own tool-call loops.
/// - Agents have system prompts and maintain internal conversation state.
/// - Agents produce structured AgentResponse (not raw JSON Value).
/// - Agents are long-running (multiple API calls), not single-shot.
#[async_trait]
pub trait Agent: Send + Sync + Debug {
    /// Unique identifier for this agent type (e.g., "explorer", "planner").
    fn agent_type(&self) -> &str;

    /// The system prompt that guides this agent's behavior.
    fn system_prompt(&self) -> &str;

    /// Maximum tool-call iterations before forced completion.
    fn max_iterations(&self) -> usize;

    /// Run the agent with the given request.
    /// The agent creates its own conversation, runs its tool loop,
    /// and returns a structured response.
    async fn run(&self, client: &GeminiClient, request: AgentRequest) -> Result<AgentResponse>;
}

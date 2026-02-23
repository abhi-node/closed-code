use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Callback invoked by sub-agents when they execute a tool.
/// Parameters: (tool_name, args_display).
pub type ToolProgressFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Request sent to a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Unique ID for tracking this request.
    pub id: Uuid,
    /// The task description (what the agent should research/plan/search for).
    pub task: String,
    /// Optional context strings from the orchestrator's conversation.
    pub context: Vec<String>,
    /// Working directory for filesystem operations.
    pub working_directory: String,
}

impl AgentRequest {
    pub fn new(task: String, working_directory: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            task,
            context: Vec::new(),
            working_directory,
        }
    }

    pub fn with_context(mut self, context: Vec<String>) -> Self {
        self.context = context;
        self
    }
}

/// Structured response from a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The request ID this response corresponds to.
    pub request_id: Uuid,
    /// Which agent produced this response.
    pub agent_type: String,
    /// A brief summary suitable for the orchestrator to reference.
    pub summary: String,
    /// The detailed findings/plan/research results.
    pub detailed_report: String,
    /// Structured artifacts (code snippets, file listings, etc.)
    pub artifacts: Vec<Artifact>,
}

/// A structured piece of output from a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Human-readable name for this artifact.
    pub name: String,
    /// Type of artifact.
    pub artifact_type: ArtifactType,
    /// The artifact's content.
    pub content: String,
}

/// Types of artifacts that sub-agents can produce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    /// A code snippet with language identifier.
    CodeSnippet { language: String },
    /// Contents of a specific file.
    FileContent { path: String },
    /// A directory listing.
    DirectoryListing,
    /// Search results (grep/file search).
    SearchResults,
    /// An implementation plan with steps.
    Plan,
    /// A diff (future use).
    Diff,
    /// Web search results with sources.
    WebSearchResults { sources: Vec<WebSource> },
}

/// A web source from grounded search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSource {
    pub url: String,
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_request_new() {
        let req = AgentRequest::new("Analyze the error handling".into(), "/tmp/project".into());
        assert_eq!(req.task, "Analyze the error handling");
        assert_eq!(req.working_directory, "/tmp/project");
        assert!(req.context.is_empty());
    }

    #[test]
    fn agent_request_with_context() {
        let req = AgentRequest::new("task".into(), "/tmp".into())
            .with_context(vec!["The user asked about error handling".into()]);
        assert_eq!(req.context.len(), 1);
    }

    #[test]
    fn agent_response_serialization_roundtrip() {
        let response = AgentResponse {
            request_id: Uuid::nil(),
            agent_type: "explorer".into(),
            summary: "Found 3 error types".into(),
            detailed_report: "Detailed analysis...".into(),
            artifacts: vec![Artifact {
                name: "error.rs".into(),
                artifact_type: ArtifactType::CodeSnippet {
                    language: "rust".into(),
                },
                content: "pub enum Error { ... }".into(),
            }],
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: AgentResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.summary, "Found 3 error types");
        assert_eq!(parsed.artifacts.len(), 1);
    }

    #[test]
    fn artifact_types() {
        let code = ArtifactType::CodeSnippet {
            language: "rust".into(),
        };
        assert!(matches!(code, ArtifactType::CodeSnippet { .. }));

        let web = ArtifactType::WebSearchResults {
            sources: vec![WebSource {
                url: "https://example.com".into(),
                title: Some("Example".into()),
            }],
        };
        assert!(matches!(web, ArtifactType::WebSearchResults { .. }));
    }
}

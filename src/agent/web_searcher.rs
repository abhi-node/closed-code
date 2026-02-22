use async_trait::async_trait;
use std::time::Duration;

use crate::agent::message::{Artifact, ArtifactType, WebSource as AgentWebSource};
use crate::agent::{Agent, AgentRequest, AgentResponse};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::*;
use crate::gemini::GeminiClient;

const WEB_SEARCH_TIMEOUT_SECS: u64 = 30;

const WEB_SEARCH_SYSTEM_PROMPT: &str = "\
You are a web research agent. Search the web to find relevant, up-to-date information \
about the given topic. Focus on:

1. Official documentation and best practices.
2. Recent blog posts, tutorials, and Stack Overflow answers.
3. Library READMEs and changelogs for version-specific info.

Synthesize findings into a clear, actionable summary. Always cite your sources. \
Present information in a structured format with headings for different aspects.";

#[derive(Debug)]
pub struct WebSearchAgent;

impl WebSearchAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebSearchAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Agent for WebSearchAgent {
    fn agent_type(&self) -> &str {
        "web_searcher"
    }

    fn system_prompt(&self) -> &str {
        WEB_SEARCH_SYSTEM_PROMPT
    }

    fn max_iterations(&self) -> usize {
        1 // Single request-response
    }

    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let mut user_message = format!("Research topic: {}\n", request.task);
        if !request.context.is_empty() {
            user_message.push_str("\nContext:\n");
            for ctx in &request.context {
                user_message.push_str(&format!("- {}\n", ctx));
            }
        }

        let api_request = GenerateContentRequest {
            contents: vec![Content::user(&user_message)],
            system_instruction: Some(Content::system(self.system_prompt())),
            generation_config: Some(GenerationConfig {
                temperature: Some(0.7),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(4096),
            }),
            // google_search grounding — NOT function calling
            tools: Some(vec![GeminiTool::GoogleSearch(GoogleSearchTool::new())]),
            tool_config: None, // No function calling config for google_search
        };

        let response = tokio::time::timeout(
            Duration::from_secs(WEB_SEARCH_TIMEOUT_SECS),
            client.generate_content(&api_request),
        )
        .await
        .map_err(|_| ClosedCodeError::AgentTimeout {
            agent_id: "web_searcher".into(),
            seconds: WEB_SEARCH_TIMEOUT_SECS,
        })??;

        let candidate = response
            .candidates
            .first()
            .ok_or(ClosedCodeError::EmptyResponse)?;

        let text = candidate
            .content
            .as_ref()
            .and_then(|c| c.parts.first())
            .and_then(|p| match p {
                Part::Text(t) => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "No results found.".into());

        // Extract grounding sources
        let sources: Vec<AgentWebSource> = candidate
            .grounding_metadata
            .as_ref()
            .map(|gm| {
                gm.grounding_chunks
                    .iter()
                    .filter_map(|chunk| {
                        chunk.web.as_ref().map(|web| AgentWebSource {
                            url: web.uri.clone(),
                            title: web.title.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let summary = if text.len() > 200 {
            format!("{}...", &text[..200])
        } else {
            text.clone()
        };

        let mut artifacts = Vec::new();
        if !sources.is_empty() {
            artifacts.push(Artifact {
                name: "Web Search Results".into(),
                artifact_type: ArtifactType::WebSearchResults {
                    sources: sources.clone(),
                },
                content: text.clone(),
            });
        }

        Ok(AgentResponse {
            request_id: request.id,
            agent_type: "web_searcher".into(),
            summary,
            detailed_report: text,
            artifacts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_agent_properties() {
        let agent = WebSearchAgent::new();
        assert_eq!(agent.agent_type(), "web_searcher");
        assert_eq!(agent.max_iterations(), 1);
        assert!(agent.system_prompt().contains("web research"));
    }
}

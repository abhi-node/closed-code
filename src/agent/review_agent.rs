use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::agent::message::{AgentResponse, Artifact, ArtifactType};
use crate::agent::{Agent, AgentRequest};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::*;
use crate::gemini::GeminiClient;
use crate::tool::registry::{create_planner_registry, ToolRegistry};

const REVIEW_MAX_ITERATIONS: usize = 15;
const REVIEW_TIMEOUT_SECS: u64 = 120;

const REVIEW_SYSTEM_PROMPT: &str = "\
You are an expert code reviewer agent. Your job is to analyze code changes \
and produce a thorough, structured code review.

You have access to filesystem tools (read_file, list_directory, search_files, grep, shell), \
a spawn_explorer tool for deep codebase research, and a create_report tool. Your workflow:

1. Study the diff provided in the task context.
2. Use filesystem tools to read related files for context — understand the broader codebase, \
   check existing patterns, look at tests, understand dependencies.
3. Use spawn_explorer for deep research if needed (e.g., understanding complex subsystems).
4. Produce a structured code review covering:
   - Summary of changes
   - Potential bugs or issues found
   - Code quality observations (naming, patterns, complexity)
   - Missing test coverage
   - Suggestions for improvement
   - Overall assessment
5. Call create_report with your review.

IMPORTANT:
- The summary should be a brief one-line assessment.
- The detailed_report should contain the full structured review.
- You MUST call create_report when done.";

#[derive(Debug)]
pub struct ReviewAgent {
    working_directory: PathBuf,
    client: Arc<GeminiClient>,
}

impl ReviewAgent {
    pub fn new(working_directory: PathBuf, client: Arc<GeminiClient>) -> Self {
        Self {
            working_directory,
            client,
        }
    }

    /// Run the sub-agent's tool-call loop.
    /// Same pattern as PlannerAgent but with review-focused system prompt.
    async fn run_subagent_loop(
        &self,
        client: &GeminiClient,
        history: &mut Vec<Content>,
        system_instruction: Content,
        tools: Option<Vec<GeminiTool>>,
        tool_config: Option<ToolConfig>,
    ) -> Result<Option<AgentResponse>> {
        let registry = create_planner_registry(
            self.working_directory.clone(),
            self.client.clone(),
        );

        for iteration in 0..self.max_iterations() {
            tracing::debug!(
                "Review agent loop iteration {}/{}",
                iteration + 1,
                self.max_iterations()
            );

            let request = GenerateContentRequest {
                contents: history.clone(),
                system_instruction: Some(system_instruction.clone()),
                generation_config: Some(GenerationConfig {
                    temperature: Some(0.7),
                    top_p: None,
                    top_k: None,
                    max_output_tokens: Some(8192),
                }),
                tools: tools.clone(),
                tool_config: tool_config.clone(),
            };

            let response = client.generate_content(&request).await?;

            let candidate = response
                .candidates
                .first()
                .ok_or(ClosedCodeError::EmptyResponse)?;
            let content = candidate
                .content
                .as_ref()
                .ok_or(ClosedCodeError::EmptyResponse)?;

            let mut function_calls = Vec::new();
            for part in &content.parts {
                if let Part::FunctionCall { name, args, .. } = part {
                    function_calls.push((name.clone(), args.clone()));
                }
            }

            if function_calls.is_empty() {
                history.push(content.clone());
                let text = content
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");

                if !text.is_empty() {
                    return Ok(Some(AgentResponse {
                        request_id: uuid::Uuid::nil(),
                        agent_type: self.agent_type().into(),
                        summary: text.chars().take(200).collect(),
                        detailed_report: text.clone(),
                        artifacts: vec![Artifact {
                            name: "Code Review".into(),
                            artifact_type: ArtifactType::Plan,
                            content: text,
                        }],
                    }));
                }
                break;
            }

            history.push(content.clone());

            let mut response_parts = Vec::new();
            for (name, args) in &function_calls {
                if name == "create_report" {
                    let report = Self::extract_report(args)?;
                    return Ok(Some(report));
                }

                let result = match registry.execute(name, args.clone()).await {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!("Review agent tool '{}' failed: {}", name, e);
                        serde_json::json!({"error": e.to_string()})
                    }
                };

                response_parts.push(Part::FunctionResponse {
                    name: name.clone(),
                    response: result,
                });
            }

            history.push(Content::function_responses(response_parts));
        }

        Ok(None)
    }

    fn extract_report(args: &serde_json::Value) -> Result<AgentResponse> {
        let summary = args["summary"]
            .as_str()
            .unwrap_or("No summary provided")
            .to_string();

        let detailed_report = args["detailed_report"]
            .as_str()
            .unwrap_or("No detailed report provided")
            .to_string();

        let mut artifacts = vec![Artifact {
            name: "Code Review".into(),
            artifact_type: ArtifactType::Plan,
            content: detailed_report.clone(),
        }];

        // Also extract code snippets if provided
        if let Some(snippets_str) = args["code_snippets"].as_str() {
            if let Ok(snippets) = serde_json::from_str::<Vec<serde_json::Value>>(snippets_str) {
                for s in &snippets {
                    if let (Some(name), Some(content)) =
                        (s["name"].as_str(), s["content"].as_str())
                    {
                        artifacts.push(Artifact {
                            name: name.to_string(),
                            artifact_type: ArtifactType::CodeSnippet {
                                language: s["language"]
                                    .as_str()
                                    .unwrap_or("text")
                                    .to_string(),
                            },
                            content: content.to_string(),
                        });
                    }
                }
            }
        } else if let Some(snippets) = args["code_snippets"].as_array() {
            for s in snippets {
                if let (Some(name), Some(content)) =
                    (s["name"].as_str(), s["content"].as_str())
                {
                    artifacts.push(Artifact {
                        name: name.to_string(),
                        artifact_type: ArtifactType::CodeSnippet {
                            language: s["language"]
                                .as_str()
                                .unwrap_or("text")
                                .to_string(),
                        },
                        content: content.to_string(),
                    });
                }
            }
        }

        Ok(AgentResponse {
            request_id: uuid::Uuid::nil(),
            agent_type: "reviewer".into(),
            summary,
            detailed_report,
            artifacts,
        })
    }
}

#[async_trait]
impl Agent for ReviewAgent {
    fn agent_type(&self) -> &str {
        "reviewer"
    }

    fn system_prompt(&self) -> &str {
        REVIEW_SYSTEM_PROMPT
    }

    fn max_iterations(&self) -> usize {
        REVIEW_MAX_ITERATIONS
    }

    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let registry = create_planner_registry(
            self.working_directory.clone(),
            self.client.clone(),
        );
        let tools = registry.to_gemini_tools(&crate::mode::Mode::Plan);
        let tool_config = Some(ToolRegistry::tool_config());
        let system_instruction = Content::system(self.system_prompt());

        let mut user_message = format!("Task: {}\n", request.task);
        if !request.context.is_empty() {
            user_message.push_str("\nContext:\n");
            for ctx in &request.context {
                user_message.push_str(&format!("{}\n", ctx));
            }
        }
        user_message.push_str(&format!(
            "\nWorking directory: {}",
            request.working_directory
        ));

        let mut history = vec![Content::user(&user_message)];

        let result = tokio::time::timeout(
            Duration::from_secs(REVIEW_TIMEOUT_SECS),
            self.run_subagent_loop(
                client,
                &mut history,
                system_instruction,
                tools,
                tool_config,
            ),
        )
        .await
        .map_err(|_| ClosedCodeError::AgentTimeout {
            agent_id: self.agent_type().into(),
            seconds: REVIEW_TIMEOUT_SECS,
        })??;

        match result {
            Some(mut response) => {
                response.request_id = request.id;
                Ok(response)
            }
            None => Ok(AgentResponse {
                request_id: request.id,
                agent_type: self.agent_type().into(),
                summary: "Review completed without structured output.".into(),
                detailed_report: "The review agent exhausted iterations without \
                    calling create_report."
                    .into(),
                artifacts: Vec::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_agent_properties() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let agent = ReviewAgent::new(PathBuf::from("/tmp"), client);
        assert_eq!(agent.agent_type(), "reviewer");
        assert_eq!(agent.max_iterations(), 15);
        assert!(agent.system_prompt().contains("code reviewer"));
    }

    #[test]
    fn extract_report_includes_review_artifact() {
        let args = serde_json::json!({
            "summary": "Generally good, minor issues",
            "detailed_report": "## Summary\nThe changes add...\n## Issues\n...",
        });
        let report = ReviewAgent::extract_report(&args).unwrap();
        assert_eq!(report.artifacts.len(), 1);
        assert!(matches!(
            report.artifacts[0].artifact_type,
            ArtifactType::Plan
        ));
        assert_eq!(report.artifacts[0].name, "Code Review");
    }

    #[test]
    fn extract_report_with_snippets() {
        let args = serde_json::json!({
            "summary": "Found a bug",
            "detailed_report": "The function has a bug...",
            "code_snippets": [
                {
                    "name": "buggy_code.rs",
                    "language": "rust",
                    "content": "fn foo() { unreachable!() }"
                }
            ]
        });
        let report = ReviewAgent::extract_report(&args).unwrap();
        assert_eq!(report.artifacts.len(), 2); // review + snippet
        assert_eq!(report.artifacts[1].name, "buggy_code.rs");
    }

    #[test]
    fn review_agent_constants() {
        assert_eq!(REVIEW_MAX_ITERATIONS, 15);
        assert_eq!(REVIEW_TIMEOUT_SECS, 120);
    }
}

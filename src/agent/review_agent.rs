use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use crate::agent::cache::{self, SubAgentCacheManager};
use crate::agent::message::{AgentResponse, Artifact, ArtifactType, ToolProgressFn};
use crate::agent::{Agent, AgentRequest};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::*;
use crate::gemini::GeminiClient;
use crate::sandbox::Sandbox;
use crate::tool::registry::{create_subagent_registry, ToolRegistry};

const REVIEW_MAX_ITERATIONS: usize = 30;

const REVIEW_SYSTEM_PROMPT: &str = "\
You are an expert code reviewer agent. Your job is to analyze code changes \
and produce a thorough, structured code review.

You have access to filesystem tools (read_file, list_directory, search_files, grep, shell) \
and a create_report tool. Your workflow:

1. Study the diff provided in the task context.
2. Use filesystem tools to read related files for context — understand the broader codebase, \
   check existing patterns, look at tests, understand dependencies.
3. Produce a structured code review covering:
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

pub struct ReviewAgent {
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
    on_tool_progress: Option<ToolProgressFn>,
    cache_manager: Option<Arc<SubAgentCacheManager>>,
}

impl std::fmt::Debug for ReviewAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewAgent")
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

impl ReviewAgent {
    pub fn new(working_directory: PathBuf, sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            working_directory,
            sandbox,
            on_tool_progress: None,
            cache_manager: None,
        }
    }

    pub fn with_progress(mut self, f: ToolProgressFn) -> Self {
        self.on_tool_progress = Some(f);
        self
    }

    pub fn with_cache_manager(mut self, cm: Arc<SubAgentCacheManager>) -> Self {
        self.cache_manager = Some(cm);
        self
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
        mut cached_content: Option<String>,
    ) -> Result<Option<AgentResponse>> {
        let registry =
            create_subagent_registry(self.working_directory.clone(), self.sandbox.clone());

        for iteration in 0..self.max_iterations() {
            tracing::debug!(
                "Review agent loop iteration {}/{}",
                iteration + 1,
                self.max_iterations()
            );

            let request = cache::build_subagent_request(
                history,
                &system_instruction,
                &tools,
                &tool_config,
                &cached_content,
            );

            let response = match client.generate_content(&request).await {
                Ok(r) => r,
                Err(e) if cached_content.is_some() && cache::is_subagent_cache_error(&e) => {
                    tracing::warn!("Review cache error, retrying without cache: {}", e);
                    cached_content = None;
                    let retry_req = cache::build_subagent_request(
                        history,
                        &system_instruction,
                        &tools,
                        &tool_config,
                        &None,
                    );
                    client.generate_content(&retry_req).await?
                }
                Err(e) => return Err(e),
            };

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
                    if let Some(ref cb) = self.on_tool_progress {
                        cb("create_report", "...");
                    }
                    let report = Self::extract_report(args)?;
                    return Ok(Some(report));
                }

                let display = crate::agent::orchestrator::format_tool_call(name, args);
                let result = match registry.execute(name, args.clone()).await {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!("Review agent tool '{}' failed: {}", name, e);
                        serde_json::json!({"error": e.to_string()})
                    }
                };
                if let Some(ref cb) = self.on_tool_progress {
                    cb(name, &display);
                }

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
                    if let (Some(name), Some(content)) = (s["name"].as_str(), s["content"].as_str())
                    {
                        artifacts.push(Artifact {
                            name: name.to_string(),
                            artifact_type: ArtifactType::CodeSnippet {
                                language: s["language"].as_str().unwrap_or("text").to_string(),
                            },
                            content: content.to_string(),
                        });
                    }
                }
            }
        } else if let Some(snippets) = args["code_snippets"].as_array() {
            for s in snippets {
                if let (Some(name), Some(content)) = (s["name"].as_str(), s["content"].as_str()) {
                    artifacts.push(Artifact {
                        name: name.to_string(),
                        artifact_type: ArtifactType::CodeSnippet {
                            language: s["language"].as_str().unwrap_or("text").to_string(),
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

    async fn run(&self, client: &GeminiClient, request: AgentRequest) -> Result<AgentResponse> {
        let registry =
            create_subagent_registry(self.working_directory.clone(), self.sandbox.clone());
        let tools = registry.to_gemini_tools(&crate::mode::Mode::Explore);
        let tool_config = Some(ToolRegistry::tool_config());
        let system_instruction = Content::system(self.system_prompt());

        let cached_content = if let Some(ref cm) = self.cache_manager {
            cache::ensure_subagent_cache(
                cm,
                client,
                self.agent_type(),
                &system_instruction,
                &tools,
                &tool_config,
            )
            .await
        } else {
            None
        };

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

        let result = self
            .run_subagent_loop(
                client,
                &mut history,
                system_instruction,
                tools,
                tool_config,
                cached_content,
            )
            .await?;

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
    use crate::sandbox::mock::MockSandbox;

    fn mock_sandbox() -> Arc<dyn Sandbox> {
        Arc::new(MockSandbox::new(PathBuf::from("/tmp")))
    }

    #[test]
    fn review_agent_properties() {
        let agent = ReviewAgent::new(PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(agent.agent_type(), "reviewer");
        assert_eq!(agent.max_iterations(), 30);
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
        assert_eq!(REVIEW_MAX_ITERATIONS, 30);
    }
}

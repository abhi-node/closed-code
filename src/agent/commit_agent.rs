use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use crate::agent::cache::{self, SubAgentCacheManager};
use crate::agent::message::{AgentResponse, ToolProgressFn};
use crate::agent::{Agent, AgentRequest};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::*;
use crate::gemini::GeminiClient;
use crate::sandbox::Sandbox;
use crate::tool::registry::{create_subagent_registry, ToolRegistry};

const COMMIT_MAX_ITERATIONS: usize = 10;

const COMMIT_SYSTEM_PROMPT: &str = "\
You are a commit message generator agent. Your job is to analyze code changes \
and generate a clear, concise git commit message.

You have access to filesystem tools (read_file, list_directory, search_files, grep, shell) \
and a create_report tool. Your workflow:

1. Study the diff provided in the task context to understand what changed.
2. Optionally use filesystem tools to read related files for better context \
   (e.g., understand what a function does, check related tests, etc.).
3. Generate a commit message following conventional commit style:
   - Subject line: max 72 chars, imperative mood (e.g., \"Add user auth flow\")
   - Optional body: explain WHY the change was made, not WHAT (the diff shows WHAT)
4. Call create_report with the commit message as the summary field.

IMPORTANT:
- The summary field of create_report MUST contain ONLY the commit message text.
- Do NOT include quotes, backticks, or prefixes like \"commit message:\" in the summary.
- The detailed_report can contain your reasoning about the changes.
- You MUST call create_report when done.";

pub struct CommitAgent {
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
    on_tool_progress: Option<ToolProgressFn>,
    cache_manager: Option<Arc<SubAgentCacheManager>>,
}

impl std::fmt::Debug for CommitAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommitAgent")
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

impl CommitAgent {
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
    /// Returns when create_report is called or max iterations reached.
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
                "Commit agent loop iteration {}/{}",
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
                    tracing::warn!("Commit cache error, retrying without cache: {}", e);
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

            // Separate text and function calls
            let mut function_calls = Vec::new();
            for part in &content.parts {
                if let Part::FunctionCall { name, args, .. } = part {
                    function_calls.push((name.clone(), args.clone()));
                }
            }

            // If no function calls, the agent is done (no create_report — fallback)
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
                        detailed_report: text,
                        artifacts: Vec::new(),
                    }));
                }
                break;
            }

            // Append model's response to history
            history.push(content.clone());

            // Execute function calls
            let mut response_parts = Vec::new();
            for (name, args) in &function_calls {
                // Check if this is a create_report call — intercept and extract
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
                        tracing::warn!("Commit agent tool '{}' failed: {}", name, e);
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

        // Max iterations reached without create_report
        tracing::warn!(
            "Commit agent exhausted {} iterations",
            self.max_iterations()
        );
        Ok(None)
    }

    /// Extract an AgentResponse from create_report tool arguments.
    pub fn extract_report(args: &serde_json::Value) -> Result<AgentResponse> {
        let summary = args["summary"]
            .as_str()
            .unwrap_or("No summary provided")
            .to_string();

        let detailed_report = args["detailed_report"]
            .as_str()
            .unwrap_or("No detailed report provided")
            .to_string();

        let artifacts = if let Some(snippets) = args["code_snippets"].as_array() {
            snippets
                .iter()
                .filter_map(|s| {
                    let name = s["name"].as_str()?.to_string();
                    let language = s["language"].as_str().unwrap_or("text").to_string();
                    let content = s["content"].as_str()?.to_string();
                    Some(crate::agent::message::Artifact {
                        name,
                        artifact_type: crate::agent::message::ArtifactType::CodeSnippet {
                            language,
                        },
                        content,
                    })
                })
                .collect()
        } else if let Some(snippets_str) = args["code_snippets"].as_str() {
            if let Ok(snippets) = serde_json::from_str::<Vec<serde_json::Value>>(snippets_str) {
                snippets
                    .iter()
                    .filter_map(|s| {
                        let name = s["name"].as_str()?.to_string();
                        let language = s["language"].as_str().unwrap_or("text").to_string();
                        let content = s["content"].as_str()?.to_string();
                        Some(crate::agent::message::Artifact {
                            name,
                            artifact_type: crate::agent::message::ArtifactType::CodeSnippet {
                                language,
                            },
                            content,
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Ok(AgentResponse {
            request_id: uuid::Uuid::nil(),
            agent_type: "commit".into(),
            summary,
            detailed_report,
            artifacts,
        })
    }
}

#[async_trait]
impl Agent for CommitAgent {
    fn agent_type(&self) -> &str {
        "commit"
    }

    fn system_prompt(&self) -> &str {
        COMMIT_SYSTEM_PROMPT
    }

    fn max_iterations(&self) -> usize {
        COMMIT_MAX_ITERATIONS
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

        // Build initial message from the request
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
            None => {
                // Agent finished without a report — produce a minimal one
                Ok(AgentResponse {
                    request_id: request.id,
                    agent_type: self.agent_type().into(),
                    summary: "Update codebase".into(),
                    detailed_report: "The commit agent could not generate a detailed message."
                        .into(),
                    artifacts: Vec::new(),
                })
            }
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
    fn commit_agent_properties() {
        let agent = CommitAgent::new(PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(agent.agent_type(), "commit");
        assert_eq!(agent.max_iterations(), 10);
        assert!(agent.system_prompt().contains("commit message"));
    }

    #[test]
    fn extract_report_basic() {
        let args = serde_json::json!({
            "summary": "Add user authentication flow",
            "detailed_report": "Implemented login/logout endpoints...",
        });
        let report = CommitAgent::extract_report(&args).unwrap();
        assert_eq!(report.summary, "Add user authentication flow");
        assert_eq!(report.agent_type, "commit");
        assert!(report.artifacts.is_empty());
    }

    #[test]
    fn extract_report_missing_fields() {
        let args = serde_json::json!({});
        let report = CommitAgent::extract_report(&args).unwrap();
        assert_eq!(report.summary, "No summary provided");
        assert_eq!(report.detailed_report, "No detailed report provided");
    }

    // Compile-time check: commit agent should be lighter than explorer
    const _: () = assert!(COMMIT_MAX_ITERATIONS < 15); // Explorer is 15
}

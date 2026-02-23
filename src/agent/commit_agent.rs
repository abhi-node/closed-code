use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

use crate::agent::message::AgentResponse;
use crate::agent::{Agent, AgentRequest};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::*;
use crate::gemini::GeminiClient;
use crate::tool::registry::{create_subagent_registry, ToolRegistry};

const COMMIT_MAX_ITERATIONS: usize = 10;
const COMMIT_TIMEOUT_SECS: u64 = 90;

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

#[derive(Debug)]
pub struct CommitAgent {
    working_directory: PathBuf,
}

impl CommitAgent {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
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
    ) -> Result<Option<AgentResponse>> {
        let registry = create_subagent_registry(self.working_directory.clone());

        for iteration in 0..self.max_iterations() {
            tracing::debug!(
                "Commit agent loop iteration {}/{}",
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
                    println!("│  \u{2713} create_report(...)");
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
                println!("│  \u{2713} {}", display);

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
            if let Ok(snippets) =
                serde_json::from_str::<Vec<serde_json::Value>>(snippets_str)
            {
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

    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let registry = create_subagent_registry(self.working_directory.clone());
        let tools = registry.to_gemini_tools(&crate::mode::Mode::Explore);
        let tool_config = Some(ToolRegistry::tool_config());
        let system_instruction = Content::system(self.system_prompt());

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

        // Run with timeout
        let result = tokio::time::timeout(
            Duration::from_secs(COMMIT_TIMEOUT_SECS),
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
            seconds: COMMIT_TIMEOUT_SECS,
        })??;

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

    #[test]
    fn commit_agent_properties() {
        let agent = CommitAgent::new(PathBuf::from("/tmp"));
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

    #[test]
    fn commit_agent_constants() {
        // Commit agent should be faster/lighter than explorer
        assert!(COMMIT_MAX_ITERATIONS < 15); // Explorer is 15
        assert!(COMMIT_TIMEOUT_SECS < 120); // Explorer is 120
    }
}

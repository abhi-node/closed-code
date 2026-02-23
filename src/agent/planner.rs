use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::agent::message::{AgentResponse, Artifact, ArtifactType};
use crate::agent::{Agent, AgentRequest};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::*;
use crate::gemini::GeminiClient;
use crate::sandbox::Sandbox;
use crate::tool::registry::{create_subagent_registry, ToolRegistry};

const PLANNER_MAX_ITERATIONS: usize = 20;
const PLANNER_TIMEOUT_SECS: u64 = 180;

const PLANNER_SYSTEM_PROMPT: &str = "\
You are an expert software architect and planning agent. Your job is to analyze a codebase \
and create detailed, actionable implementation plans.

You have access to filesystem tools (read_file, list_directory, search_files, grep, shell) \
and a create_report tool. Your workflow:

1. Use the filesystem tools to understand the codebase structure and patterns.
2. Read relevant files, search for patterns, and trace dependencies.
3. Identify affected files and potential challenges.
4. Create a structured plan with clear steps.
5. Call create_report with your plan.

Your plan should include:
- Step-by-step implementation order (numbered)
- Files to create or modify (with rationale)
- Key code patterns to follow (from existing codebase)
- Potential risks or trade-offs
- Estimated complexity per step

IMPORTANT: You MUST call create_report when done. The summary should be a brief overview \
of the plan. The detailed_report should contain the full plan. Include code snippets \
showing proposed implementations or patterns to follow.";

#[derive(Debug)]
pub struct PlannerAgent {
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
}

impl PlannerAgent {
    pub fn new(working_directory: PathBuf, sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            working_directory,
            sandbox,
        }
    }

    /// Run the sub-agent's tool-call loop.
    /// Same pattern as ExplorerAgent but with spawn_explorer in its registry.
    async fn run_subagent_loop(
        &self,
        client: &GeminiClient,
        history: &mut Vec<Content>,
        system_instruction: Content,
        tools: Option<Vec<GeminiTool>>,
        tool_config: Option<ToolConfig>,
    ) -> Result<Option<AgentResponse>> {
        let registry = create_subagent_registry(self.working_directory.clone(), self.sandbox.clone());

        for iteration in 0..self.max_iterations() {
            tracing::debug!(
                "Planner agent loop iteration {}/{}",
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
                            name: "Implementation Plan".into(),
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
                    println!("│  \u{2713} create_report(...)");
                    let report = Self::extract_report(args)?;
                    return Ok(Some(report));
                }

                let display = crate::agent::orchestrator::format_tool_call(name, args);
                let result = match registry.execute(name, args.clone()).await {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!("Planner tool '{}' failed: {}", name, e);
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
            name: "Implementation Plan".into(),
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
            agent_type: "planner".into(),
            summary,
            detailed_report,
            artifacts,
        })
    }
}

#[async_trait]
impl Agent for PlannerAgent {
    fn agent_type(&self) -> &str {
        "planner"
    }

    fn system_prompt(&self) -> &str {
        PLANNER_SYSTEM_PROMPT
    }

    fn max_iterations(&self) -> usize {
        PLANNER_MAX_ITERATIONS
    }

    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let registry = create_subagent_registry(self.working_directory.clone(), self.sandbox.clone());
        let tools = registry.to_gemini_tools(&crate::mode::Mode::Explore);
        let tool_config = Some(ToolRegistry::tool_config());
        let system_instruction = Content::system(self.system_prompt());

        let mut user_message = format!("Task: {}\n", request.task);
        if !request.context.is_empty() {
            user_message.push_str("\nContext:\n");
            for ctx in &request.context {
                user_message.push_str(&format!("- {}\n", ctx));
            }
        }
        user_message.push_str(&format!(
            "\nWorking directory: {}",
            request.working_directory
        ));

        let mut history = vec![Content::user(&user_message)];

        let result = tokio::time::timeout(
            Duration::from_secs(PLANNER_TIMEOUT_SECS),
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
            seconds: PLANNER_TIMEOUT_SECS,
        })??;

        match result {
            Some(mut response) => {
                response.request_id = request.id;
                Ok(response)
            }
            None => Ok(AgentResponse {
                request_id: request.id,
                agent_type: self.agent_type().into(),
                summary: "Planner completed without a structured plan.".into(),
                detailed_report: "The planner exhausted iterations without \
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
    fn planner_agent_properties() {
        let agent = PlannerAgent::new(PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(agent.agent_type(), "planner");
        assert_eq!(agent.max_iterations(), 20);
        assert!(agent.system_prompt().contains("software architect"));
    }

    #[test]
    fn extract_report_includes_plan_artifact() {
        let args = serde_json::json!({
            "summary": "3-step plan",
            "detailed_report": "Step 1: ...\nStep 2: ...\nStep 3: ...",
        });
        let report = PlannerAgent::extract_report(&args).unwrap();
        assert_eq!(report.artifacts.len(), 1);
        assert!(matches!(
            report.artifacts[0].artifact_type,
            ArtifactType::Plan
        ));
    }

    #[test]
    fn planner_has_higher_timeout_than_explorer() {
        assert!(PLANNER_TIMEOUT_SECS > 120); // Explorer is 120s
        assert!(PLANNER_MAX_ITERATIONS > 15); // Explorer is 15
    }
}

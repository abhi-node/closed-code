use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::error::Result;
use crate::gemini::stream::{consume_stream, StreamEvent, StreamResult};
use crate::gemini::types::{Content, GenerateContentRequest, GenerationConfig, Part};
use crate::gemini::GeminiClient;
use crate::mode::Mode;
use crate::tool::registry::{create_orchestrator_registry, ToolRegistry};
use crate::ui::spinner::Spinner;

const MAX_ORCHESTRATOR_ITERATIONS: usize = 30;
const MAX_CONTEXT_TURNS: usize = 50;

/// The main orchestrator that owns the Gemini client, tool registry,
/// conversation history, and mode-specific system prompt.
///
/// The REPL creates one Orchestrator and delegates all user input through it.
pub struct Orchestrator {
    client: Arc<GeminiClient>,
    mode: Mode,
    #[allow(dead_code)]
    working_directory: PathBuf,
    history: Vec<Content>,
    registry: ToolRegistry,
    system_prompt: String,
    max_output_tokens: u32,
}

impl Orchestrator {
    pub fn new(
        client: Arc<GeminiClient>,
        mode: Mode,
        working_directory: PathBuf,
        max_output_tokens: u32,
    ) -> Self {
        let registry =
            create_orchestrator_registry(working_directory.clone(), &mode, client.clone());
        let system_prompt = Self::build_system_prompt(&mode, &working_directory);

        Self {
            client,
            mode,
            working_directory,
            history: Vec::new(),
            registry,
            system_prompt,
            max_output_tokens,
        }
    }

    /// Handle user input with streaming callbacks for real-time display.
    ///
    /// Adds the user message to history, streams the Gemini response,
    /// executes any function calls, and returns the final assistant text.
    pub async fn handle_user_input_streaming(
        &mut self,
        input: &str,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<String> {
        self.history.push(Content::user(input));
        self.prune_history();

        let request = self.build_request();

        let spinner = Spinner::new("Thinking...");
        let es = self.client.stream_generate_content(&request);
        let mut spinner_cleared = false;

        let stream_result = consume_stream(es, |event| {
            if !spinner_cleared {
                spinner.finish();
                spinner_cleared = true;
            }
            on_event(event);
        })
        .await?;

        if !spinner_cleared {
            spinner.finish();
        }

        match stream_result {
            StreamResult::Text(text) => {
                self.history.push(Content::model(&text));
                Ok(text)
            }
            StreamResult::FunctionCall {
                text_so_far,
                response,
            } => {
                // Append model's function call content to history
                if let Some(candidate) = response.candidates.first() {
                    if let Some(content) = &candidate.content {
                        self.history.push(content.clone());
                    }
                }

                // Execute the initial function calls
                let mut response_parts = Vec::new();
                for part in response.function_calls() {
                    if let Part::FunctionCall { name, args, .. } = part {
                        let display = format_tool_call(name, args);
                        let tool_spinner = Spinner::new(&format!("[tool] {}", display));

                        let result = match self.registry.execute(name, args.clone()).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("Tool '{}' failed: {}", name, e);
                                serde_json::json!({"error": e.to_string()})
                            }
                        };

                        tool_spinner.finish_with_message(&format!("\u{2713} [tool] {}", display));
                        response_parts.push(Part::FunctionResponse {
                            name: name.clone(),
                            response: result,
                        });
                    }
                }

                self.history
                    .push(Content::function_responses(response_parts));

                // Continue with the tool loop
                let loop_text = self.run_tool_loop(&mut on_event).await?;
                let mut final_text = text_so_far;
                final_text.push_str(&loop_text);
                Ok(final_text)
            }
        }
    }

    /// The streaming tool-call loop.
    ///
    /// Sends requests to Gemini, executes function calls, and repeats
    /// until a text-only response or max iterations.
    async fn run_tool_loop(
        &mut self,
        on_event: &mut impl FnMut(StreamEvent),
    ) -> Result<String> {
        let mut final_text = String::new();

        for iteration in 0..MAX_ORCHESTRATOR_ITERATIONS {
            tracing::debug!(
                "Orchestrator tool loop iteration {}/{}",
                iteration + 1,
                MAX_ORCHESTRATOR_ITERATIONS
            );

            let request = self.build_request();

            let spinner = Spinner::new("Thinking...");
            let es = self.client.stream_generate_content(&request);
            let mut spinner_cleared = false;

            let stream_result = consume_stream(es, |event| {
                if !spinner_cleared {
                    spinner.finish();
                    spinner_cleared = true;
                }
                on_event(event);
            })
            .await?;

            if !spinner_cleared {
                spinner.finish();
            }

            match stream_result {
                StreamResult::Text(text) => {
                    final_text.push_str(&text);
                    self.history.push(Content::model(&text));
                    break;
                }
                StreamResult::FunctionCall {
                    text_so_far,
                    response,
                } => {
                    final_text.push_str(&text_so_far);
                    if !text_so_far.is_empty() {
                        println!();
                    }

                    // Append model's function call content to history
                    if let Some(candidate) = response.candidates.first() {
                        if let Some(content) = &candidate.content {
                            self.history.push(content.clone());
                        }
                    }

                    // Execute all function calls
                    let mut response_parts: Vec<Part> = Vec::new();
                    for part in response.function_calls() {
                        if let Part::FunctionCall { name, args, .. } = part {
                            let display = format_tool_call(name, args);
                            let tool_spinner =
                                Spinner::new(&format!("[tool] {}", display));

                            let result =
                                match self.registry.execute(name, args.clone()).await {
                                    Ok(v) => v,
                                    Err(e) => {
                                        tracing::warn!("Tool '{}' failed: {}", name, e);
                                        serde_json::json!({"error": e.to_string()})
                                    }
                                };

                            tool_spinner
                                .finish_with_message(&format!("\u{2713} [tool] {}", display));
                            response_parts.push(Part::FunctionResponse {
                                name: name.clone(),
                                response: result,
                            });
                        }
                    }

                    self.history
                        .push(Content::function_responses(response_parts));
                }
            }
        }

        if final_text.is_empty() {
            tracing::warn!(
                "Orchestrator tool loop exhausted {} iterations without final text",
                MAX_ORCHESTRATOR_ITERATIONS
            );
        }

        Ok(final_text)
    }

    /// Build a GenerateContentRequest from current state.
    fn build_request(&self) -> GenerateContentRequest {
        let tools = self.registry.to_gemini_tools(&self.mode);
        let tool_config = tools.as_ref().map(|_| ToolRegistry::tool_config());

        GenerateContentRequest {
            contents: self.history.clone(),
            system_instruction: Some(Content::system(&self.system_prompt)),
            generation_config: Some(GenerationConfig {
                temperature: Some(1.0),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(self.max_output_tokens),
            }),
            tools,
            tool_config,
        }
    }

    /// Build the mode-specific system prompt.
    fn build_system_prompt(mode: &Mode, working_directory: &std::path::Path) -> String {
        let base = format!(
            "You are closed-code, an AI coding assistant operating in {} mode.\n\
             Working directory: {}",
            mode,
            working_directory.display()
        );

        let tools_section = match mode {
            Mode::Explore => {
                "\n\nYou have access to filesystem tools and a spawn_explorer tool.\n\
                 Use spawn_explorer for deep codebase research when you need a thorough analysis."
            }
            Mode::Plan => {
                "\n\nYou have access to filesystem tools and these agent tools:\n\
                 - spawn_explorer: Deep codebase research and analysis\n\
                 - spawn_planner: Create detailed implementation plans\n\
                 - spawn_web_search: Research topics online with Google Search\n\
                 Use these tools to gather information before providing your response."
            }
            Mode::Execute => {
                "\n\nYou have access to filesystem tools and a spawn_explorer tool.\n\
                 Use spawn_explorer when you need to understand code before making changes."
            }
        };

        format!("{}{}", base, tools_section)
    }

    /// Prune conversation history when it exceeds MAX_CONTEXT_TURNS.
    /// Drops the oldest half, ensuring the first entry has role "user".
    pub fn prune_history(&mut self) {
        if self.history.len() <= MAX_CONTEXT_TURNS {
            return;
        }

        let keep = self.history.len() / 2;
        self.history = self.history.split_off(self.history.len() - keep);

        // Ensure the first message is from the user
        let first_is_user = self
            .history
            .first()
            .and_then(|c| c.role.as_deref())
            .map(|r| r == "user")
            .unwrap_or(false);

        if !first_is_user {
            self.history.insert(
                0,
                Content::user("[Earlier conversation context was pruned]"),
            );
        }
    }

    /// Clear the conversation history.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Current mode.
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    /// Number of conversation turns.
    pub fn turn_count(&self) -> usize {
        self.history.len()
    }

    /// Reference to the system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("mode", &self.mode)
            .field("tools", &self.registry.len())
            .field("history_len", &self.history.len())
            .finish()
    }
}

/// Format a tool call for display: `tool_name(key: "value", key2: 123)`
pub(crate) fn format_tool_call(name: &str, args: &Value) -> String {
    let params = if let Some(obj) = args.as_object() {
        obj.iter()
            .map(|(k, v)| {
                let display_val = match v {
                    Value::String(s) => {
                        if s.len() > 60 {
                            format!("\"{}...\"", &s[..57])
                        } else {
                            format!("\"{}\"", s)
                        }
                    }
                    other => {
                        let s = other.to_string();
                        if s.len() > 60 {
                            format!("{}...", &s[..57])
                        } else {
                            s
                        }
                    }
                };
                format!("{}: {}", k, display_val)
            })
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        String::new()
    };

    format!("{}({})", name, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Arc<GeminiClient> {
        Arc::new(GeminiClient::new("key".into(), "model".into()))
    }

    #[test]
    fn orchestrator_new_explore_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
        );
        // 5 filesystem/shell + spawn_explorer = 6
        assert_eq!(orch.tool_count(), 6);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert!(orch.system_prompt().contains("explore"));
        assert!(orch.system_prompt().contains("spawn_explorer"));
        assert!(!orch.system_prompt().contains("spawn_planner"));
    }

    #[test]
    fn orchestrator_new_plan_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
        );
        // 5 filesystem/shell + spawn_explorer + spawn_planner + spawn_web_search = 8
        assert_eq!(orch.tool_count(), 8);
        assert_eq!(*orch.mode(), Mode::Plan);
        assert!(orch.system_prompt().contains("plan"));
        assert!(orch.system_prompt().contains("spawn_explorer"));
        assert!(orch.system_prompt().contains("spawn_planner"));
        assert!(orch.system_prompt().contains("spawn_web_search"));
    }

    #[test]
    fn orchestrator_clear_history() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
        );
        // Simulate some conversation
        orch.history.push(Content::user("hello"));
        orch.history.push(Content::model("hi there"));
        assert_eq!(orch.turn_count(), 2);

        orch.clear_history();
        assert_eq!(orch.turn_count(), 0);
        assert!(orch.history.is_empty());
    }

    #[test]
    fn orchestrator_prune_history() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
        );

        // Fill history beyond MAX_CONTEXT_TURNS
        for i in 0..60 {
            if i % 2 == 0 {
                orch.history.push(Content::user(&format!("msg {}", i)));
            } else {
                orch.history.push(Content::model(&format!("reply {}", i)));
            }
        }
        assert_eq!(orch.turn_count(), 60);

        orch.prune_history();
        assert!(orch.turn_count() <= MAX_CONTEXT_TURNS);

        // First entry should be role "user"
        let first_role = orch.history[0].role.as_deref();
        assert_eq!(first_role, Some("user"));
    }

    #[test]
    fn orchestrator_prune_no_op_when_small() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
        );
        orch.history.push(Content::user("hello"));
        orch.history.push(Content::model("hi"));
        assert_eq!(orch.turn_count(), 2);

        orch.prune_history();
        assert_eq!(orch.turn_count(), 2);
    }

    #[test]
    fn orchestrator_debug_format() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
        );
        let debug = format!("{:?}", orch);
        assert!(debug.contains("Orchestrator"));
        assert!(debug.contains("Explore"));
    }

    #[test]
    fn format_tool_call_basic() {
        let args = serde_json::json!({"path": "src/main.rs"});
        let result = format_tool_call("read_file", &args);
        assert!(result.starts_with("read_file("));
        assert!(result.contains("path:"));
        assert!(result.contains("src/main.rs"));
        assert!(result.ends_with(')'));
    }

    #[test]
    fn format_tool_call_empty_args() {
        let args = serde_json::json!({});
        let result = format_tool_call("list_directory", &args);
        assert_eq!(result, "list_directory()");
    }

    #[test]
    fn format_tool_call_truncates_long_strings() {
        let long_val = "a".repeat(100);
        let args = serde_json::json!({"content": long_val});
        let result = format_tool_call("write_file", &args);
        assert!(result.contains("..."));
    }

    #[test]
    fn max_orchestrator_iterations_constant() {
        assert_eq!(MAX_ORCHESTRATOR_ITERATIONS, 30);
    }

    #[test]
    fn max_context_turns_constant() {
        assert_eq!(MAX_CONTEXT_TURNS, 50);
    }
}

use std::io::Write;

use crossterm::style::Stylize;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use serde_json::Value;

use crate::config::Config;
use crate::error::Result;
use crate::gemini::stream::{consume_stream, StreamEvent, StreamResult};
use crate::gemini::types::{
    Content, GenerateContentRequest, GenerationConfig, Part, ToolConfig, ToolDefinition,
};
use crate::gemini::GeminiClient;
use crate::tool::registry::{create_default_registry, ToolRegistry};
use crate::ui::spinner::Spinner;
use crate::ui::theme::Theme;

const MAX_TOOL_ITERATIONS: usize = 10;

fn styled_text(text: &str, color: crossterm::style::Color) -> String {
    text.with(color).to_string()
}

/// Format a tool call for display: `tool_name(key: "value", key2: 123)`
fn format_tool_call(name: &str, args: &Value) -> String {
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

fn build_system_prompt(config: &Config) -> String {
    format!(
        "You are closed-code, an AI coding assistant. \
         You are operating in {} mode. \
         Working directory: {}",
        config.mode,
        config.working_directory.display()
    )
}

#[allow(clippy::too_many_arguments)]
/// Run the streaming tool-call loop.
///
/// Sends the request to Gemini via streaming. If the response contains function calls,
/// executes them, sends the results back, and repeats. Continues until
/// Gemini responds with text only, or max iterations are reached.
///
/// Manages its own spinner display: "Thinking..." while waiting for tokens,
/// "[tool] name(args)" while executing tools.
///
/// Returns the final assistant text for appending to conversation history.
async fn run_tool_loop(
    client: &GeminiClient,
    registry: &ToolRegistry,
    history: &mut Vec<Content>,
    system_instruction: Option<Content>,
    generation_config: Option<GenerationConfig>,
    tools: Option<Vec<ToolDefinition>>,
    tool_config: Option<ToolConfig>,
) -> Result<String> {
    let mut final_text = String::new();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        tracing::debug!(
            "Tool loop iteration {}/{}",
            iteration + 1,
            MAX_TOOL_ITERATIONS
        );

        let request = GenerateContentRequest {
            contents: history.clone(),
            system_instruction: system_instruction.clone(),
            generation_config: generation_config.clone(),
            tools: tools.clone(),
            tool_config: tool_config.clone(),
        };

        let spinner = Spinner::new("Thinking...");
        let es = client.stream_generate_content(&request);
        let mut spinner_cleared = false;

        let stream_result = consume_stream(es, |event| {
            if !spinner_cleared {
                spinner.finish();
                spinner_cleared = true;
            }
            match event {
                StreamEvent::TextDelta(text) => {
                    print!("{}", text);
                    std::io::stdout().flush().ok();
                }
                StreamEvent::Done { .. } => {}
                _ => {}
            }
        })
        .await?;

        if !spinner_cleared {
            spinner.finish();
        }

        match stream_result {
            StreamResult::Text(text) => {
                final_text.push_str(&text);
                history.push(Content::model(&text));
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
                        history.push(content.clone());
                    }
                }

                // Execute all function calls
                let mut response_parts: Vec<Part> = Vec::new();
                for part in response.function_calls() {
                    if let Part::FunctionCall { name, args, .. } = part {
                        let display = format_tool_call(name, args);
                        let tool_spinner = Spinner::new(&format!("[tool] {}", display));

                        let result = match registry.execute(name, args.clone()).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("Tool '{}' failed: {}", name, e);
                                serde_json::json!({"error": e.to_string()})
                            }
                        };

                        tool_spinner.finish_with_message(&format!("✓ [tool] {}", display));
                        response_parts.push(Part::FunctionResponse {
                            name: name.clone(),
                            response: result,
                        });
                    }
                }

                history.push(Content::function_responses(response_parts));
                // Loop continues — next iteration shows "Thinking..." again
            }
        }
    }

    if final_text.is_empty() {
        tracing::warn!(
            "Tool loop exhausted {} iterations without final text",
            MAX_TOOL_ITERATIONS
        );
    }

    Ok(final_text)
}

#[allow(clippy::too_many_arguments)]
/// Execute initial function calls from a streaming response, then enter the tool loop.
async fn handle_function_calls(
    client: &GeminiClient,
    registry: &ToolRegistry,
    history: &mut Vec<Content>,
    response: &crate::gemini::types::GenerateContentResponse,
    system_instruction: Option<Content>,
    generation_config: Option<GenerationConfig>,
    tools: Option<Vec<ToolDefinition>>,
    tool_config: Option<ToolConfig>,
) -> Result<String> {
    // Append model's function call content to history
    if let Some(candidate) = response.candidates.first() {
        if let Some(content) = &candidate.content {
            history.push(content.clone());
        }
    }

    // Execute each function call from the streaming response
    let mut response_parts = Vec::new();
    for part in response.function_calls() {
        if let Part::FunctionCall { name, args, .. } = part {
            let display = format_tool_call(name, args);
            let spinner = Spinner::new(&format!("[tool] {}", display));

            let result = match registry.execute(name, args.clone()).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Tool '{}' failed: {}", name, e);
                    serde_json::json!({"error": e.to_string()})
                }
            };

            spinner.finish_with_message(&format!("✓ [tool] {}", display));
            response_parts.push(Part::FunctionResponse {
                name: name.clone(),
                response: result,
            });
        }
    }

    history.push(Content::function_responses(response_parts));

    // Continue with streaming tool loop
    run_tool_loop(
        client,
        registry,
        history,
        system_instruction,
        generation_config,
        tools,
        tool_config,
    )
    .await
}

pub async fn run_oneshot(config: &Config, question: &str) -> anyhow::Result<()> {
    let client = GeminiClient::new(config.api_key.clone(), config.model.clone());
    let registry = create_default_registry(config.working_directory.clone());
    let system_prompt = build_system_prompt(config);

    let tools = registry.to_gemini_tools(&config.mode);
    let tool_config = tools.as_ref().map(|_| ToolRegistry::tool_config());

    let mut history = vec![Content::user(question)];

    let request = GenerateContentRequest {
        contents: history.clone(),
        system_instruction: Some(Content::system(&system_prompt)),
        generation_config: Some(GenerationConfig {
            temperature: Some(1.0),
            top_p: None,
            top_k: None,
            max_output_tokens: Some(config.max_output_tokens),
        }),
        tools: tools.clone(),
        tool_config: tool_config.clone(),
    };

    let spinner = Spinner::new("Thinking...");
    let es = client.stream_generate_content(&request);
    let mut thinking_done = false;

    let stream_result = consume_stream(es, |event| {
        if !thinking_done {
            spinner.finish();
            thinking_done = true;
        }
        match event {
            StreamEvent::TextDelta(text) => {
                print!("{}", text);
                std::io::stdout().flush().ok();
            }
            StreamEvent::Done { .. } => {
                println!();
            }
            _ => {}
        }
    })
    .await;

    if !thinking_done {
        spinner.finish();
    }

    match stream_result {
        Ok(StreamResult::Text(_)) => {}
        Ok(StreamResult::FunctionCall {
            text_so_far,
            response,
        }) => {
            if !text_so_far.is_empty() {
                println!();
            }
            match handle_function_calls(
                &client,
                &registry,
                &mut history,
                &response,
                Some(Content::system(&system_prompt)),
                Some(GenerationConfig {
                    temperature: Some(1.0),
                    top_p: None,
                    top_k: None,
                    max_output_tokens: Some(config.max_output_tokens),
                }),
                tools,
                tool_config,
            )
            .await
            {
                Ok(_text) => {
                    println!();
                }
                Err(e) => {
                    eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
                }
            }
        }
        Err(e) => {
            eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
        }
    }

    Ok(())
}

pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    let client = GeminiClient::new(config.api_key.clone(), config.model.clone());
    let registry = create_default_registry(config.working_directory.clone());
    let mut history: Vec<Content> = Vec::new();
    let mut editor = DefaultEditor::new()?;

    let system_prompt = build_system_prompt(config);
    let tools = registry.to_gemini_tools(&config.mode);
    let tool_config = tools.as_ref().map(|_| ToolRegistry::tool_config());

    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!(
        "Mode: {} | Model: {} | Tools: {}",
        config.mode,
        config.model,
        registry.len()
    );
    println!("Type /help for commands, /quit to exit.\n");

    loop {
        let prompt = format!("{} > ", config.mode);
        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);

                if line.starts_with('/') {
                    match handle_slash_command(line, &mut history) {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                    }
                }

                history.push(Content::user(line));

                let request = GenerateContentRequest {
                    contents: history.clone(),
                    system_instruction: Some(Content::system(&system_prompt)),
                    generation_config: Some(GenerationConfig {
                        temperature: Some(1.0),
                        top_p: None,
                        top_k: None,
                        max_output_tokens: Some(config.max_output_tokens),
                    }),
                    tools: tools.clone(),
                    tool_config: tool_config.clone(),
                };

                let spinner = Spinner::new("Thinking...");
                let es = client.stream_generate_content(&request);
                let mut thinking_done = false;

                let stream_result = consume_stream(es, |event| {
                    if !thinking_done {
                        spinner.finish();
                        thinking_done = true;
                    }
                    match event {
                        StreamEvent::TextDelta(text) => {
                            print!("{}", text);
                            std::io::stdout().flush().ok();
                        }
                        StreamEvent::Done { usage, .. } => {
                            println!();
                            if let Some(u) = usage {
                                tracing::debug!(
                                    "Tokens: {} prompt + {} completion = {} total",
                                    u.prompt_token_count.unwrap_or(0),
                                    u.candidates_token_count.unwrap_or(0),
                                    u.total_token_count.unwrap_or(0),
                                );
                            }
                        }
                        _ => {}
                    }
                })
                .await;

                if !thinking_done {
                    spinner.finish();
                }

                match stream_result {
                    Ok(StreamResult::Text(text)) => {
                        history.push(Content::model(&text));
                    }
                    Ok(StreamResult::FunctionCall {
                        text_so_far,
                        response,
                    }) => {
                        if !text_so_far.is_empty() {
                            println!();
                        }

                        match handle_function_calls(
                            &client,
                            &registry,
                            &mut history,
                            &response,
                            Some(Content::system(&system_prompt)),
                            Some(GenerationConfig {
                                temperature: Some(1.0),
                                top_p: None,
                                top_k: None,
                                max_output_tokens: Some(config.max_output_tokens),
                            }),
                            tools.clone(),
                            tool_config.clone(),
                        )
                        .await
                        {
                            Ok(_text) => {
                                println!();
                            }
                            Err(e) => {
                                eprintln!(
                                    "\n{}: {}",
                                    styled_text("Error", Theme::ERROR),
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
                    }
                }
                println!();
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(e) => {
                eprintln!("Input error: {}", e);
                break;
            }
        }
    }

    Ok(())
}

enum SlashResult {
    Continue,
    Quit,
}

fn handle_slash_command(input: &str, history: &mut Vec<Content>) -> SlashResult {
    match input {
        "/quit" | "/exit" | "/q" => SlashResult::Quit,
        "/clear" => {
            history.clear();
            println!("Conversation history cleared.");
            SlashResult::Continue
        }
        "/help" => {
            println!("Commands:");
            println!("  /help   — Show this help");
            println!("  /clear  — Clear conversation history");
            println!("  /quit   — Exit");
            SlashResult::Continue
        }
        _ => {
            println!(
                "Unknown command: {}. Type /help for available commands.",
                input
            );
            SlashResult::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_quit_returns_quit() {
        let mut history = vec![];
        assert!(matches!(
            handle_slash_command("/quit", &mut history),
            SlashResult::Quit
        ));
        assert!(matches!(
            handle_slash_command("/exit", &mut history),
            SlashResult::Quit
        ));
        assert!(matches!(
            handle_slash_command("/q", &mut history),
            SlashResult::Quit
        ));
    }

    #[test]
    fn slash_clear_clears_history() {
        let mut history = vec![Content::user("test"), Content::model("response")];
        let result = handle_slash_command("/clear", &mut history);
        assert!(matches!(result, SlashResult::Continue));
        assert!(history.is_empty());
    }

    #[test]
    fn slash_help_returns_continue() {
        let mut history = vec![];
        assert!(matches!(
            handle_slash_command("/help", &mut history),
            SlashResult::Continue
        ));
    }

    #[test]
    fn unknown_command_returns_continue() {
        let mut history = vec![];
        assert!(matches!(
            handle_slash_command("/unknown", &mut history),
            SlashResult::Continue
        ));
    }

    #[test]
    fn build_system_prompt_contains_mode_and_dir() {
        let config = Config {
            api_key: "key".into(),
            model: "model".into(),
            mode: crate::mode::Mode::Explore,
            working_directory: "/tmp/project".into(),
            verbose: false,
            max_output_tokens: 8192,
        };
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("explore"));
        assert!(prompt.contains("/tmp/project"));
    }

    #[test]
    fn styled_text_produces_non_empty_output() {
        let result = styled_text("test", Theme::ACCENT);
        assert!(!result.is_empty());
    }

    #[test]
    fn tool_loop_max_iterations_constant() {
        assert_eq!(MAX_TOOL_ITERATIONS, 10);
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
    fn format_tool_call_multiple_args() {
        let args = serde_json::json!({"pattern": "TODO", "case_insensitive": true});
        let result = format_tool_call("grep", &args);
        assert!(result.starts_with("grep("));
        assert!(result.contains("pattern:"));
        assert!(result.contains("case_insensitive:"));
        assert!(result.ends_with(')'));
    }

    #[test]
    fn format_tool_call_truncates_long_strings() {
        let long_val = "a".repeat(100);
        let args = serde_json::json!({"content": long_val});
        let result = format_tool_call("write_file", &args);
        assert!(result.contains("..."));
    }

    #[test]
    fn format_tool_call_non_object_args() {
        let args = serde_json::json!("just a string");
        let result = format_tool_call("some_tool", &args);
        assert_eq!(result, "some_tool()");
    }
}

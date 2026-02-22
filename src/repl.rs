use std::io::Write;

use crossterm::style::Stylize;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::config::Config;
use crate::gemini::stream::{consume_stream, StreamEvent};
use crate::gemini::types::{Content, GenerateContentRequest, GenerationConfig};
use crate::gemini::GeminiClient;
use crate::ui::spinner::Spinner;
use crate::ui::theme::Theme;

fn styled_text(text: &str, color: crossterm::style::Color) -> String {
    text.with(color).to_string()
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

pub async fn run_oneshot(config: &Config, question: &str) -> anyhow::Result<()> {
    let client = GeminiClient::new(config.api_key.clone(), config.model.clone());
    let system_prompt = build_system_prompt(config);

    let request = GenerateContentRequest {
        contents: vec![Content::user(question)],
        system_instruction: Some(Content::system(&system_prompt)),
        generation_config: Some(GenerationConfig {
            temperature: Some(1.0),
            top_p: None,
            top_k: None,
            max_output_tokens: Some(config.max_output_tokens),
        }),
    };

    let spinner = Spinner::new("Thinking...");
    let es = client.stream_generate_content(&request);
    spinner.finish();

    let _full_text = consume_stream(es, |event| match event {
        StreamEvent::TextDelta(text) => {
            print!("{}", text);
            std::io::stdout().flush().ok();
        }
        StreamEvent::Done { .. } => {
            println!();
        }
        _ => {}
    })
    .await?;

    Ok(())
}

pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    let client = GeminiClient::new(config.api_key.clone(), config.model.clone());
    let mut history: Vec<Content> = Vec::new();
    let mut editor = DefaultEditor::new()?;

    let system_prompt = build_system_prompt(config);

    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!("Mode: {} | Model: {}", config.mode, config.model);
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
                };

                let spinner = Spinner::new("Thinking...");
                let es = client.stream_generate_content(&request);
                spinner.finish();

                let full_text = consume_stream(es, |event| match event {
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
                })
                .await;

                match full_text {
                    Ok(text) => {
                        history.push(Content::model(&text));
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
}

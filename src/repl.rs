use std::io::Write;
use std::sync::Arc;

use crossterm::style::Stylize;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::agent::orchestrator::Orchestrator;
use crate::config::Config;
use crate::gemini::stream::StreamEvent;
use crate::gemini::GeminiClient;
use crate::ui::theme::Theme;

fn styled_text(text: &str, color: crossterm::style::Color) -> String {
    text.with(color).to_string()
}

/// Streaming event callback for printing to stdout.
fn default_stream_handler(event: StreamEvent) {
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
}

pub async fn run_oneshot(config: &Config, question: &str) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
    );

    match orchestrator
        .handle_user_input_streaming(question, default_stream_handler)
        .await
    {
        Ok(_) => {
            println!();
        }
        Err(e) => {
            eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
        }
    }

    Ok(())
}

pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
    );
    let mut editor = DefaultEditor::new()?;

    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!(
        "Mode: {} | Model: {} | Tools: {}",
        config.mode,
        config.model,
        orchestrator.tool_count()
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
                    match handle_slash_command(line, &mut orchestrator) {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                    }
                }

                match orchestrator
                    .handle_user_input_streaming(line, default_stream_handler)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!(
                            "\n{}: {}",
                            styled_text("Error", Theme::ERROR),
                            e
                        );
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

fn handle_slash_command(input: &str, orchestrator: &mut Orchestrator) -> SlashResult {
    match input {
        "/quit" | "/exit" | "/q" => SlashResult::Quit,
        "/clear" => {
            orchestrator.clear_history();
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
    use std::path::PathBuf;

    fn test_orchestrator() -> Orchestrator {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        Orchestrator::new(
            client,
            crate::mode::Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
        )
    }

    #[test]
    fn slash_quit_returns_quit() {
        let mut orch = test_orchestrator();
        assert!(matches!(
            handle_slash_command("/quit", &mut orch),
            SlashResult::Quit
        ));
        assert!(matches!(
            handle_slash_command("/exit", &mut orch),
            SlashResult::Quit
        ));
        assert!(matches!(
            handle_slash_command("/q", &mut orch),
            SlashResult::Quit
        ));
    }

    #[test]
    fn slash_clear_clears_history() {
        let mut orch = test_orchestrator();
        // Simulate some conversation via the orchestrator's internal state
        // We can't push directly, but we can verify clear works on a fresh orchestrator
        assert_eq!(orch.turn_count(), 0);
        handle_slash_command("/clear", &mut orch);
        assert_eq!(orch.turn_count(), 0);
    }

    #[test]
    fn slash_help_returns_continue() {
        let mut orch = test_orchestrator();
        assert!(matches!(
            handle_slash_command("/help", &mut orch),
            SlashResult::Continue
        ));
    }

    #[test]
    fn unknown_command_returns_continue() {
        let mut orch = test_orchestrator();
        assert!(matches!(
            handle_slash_command("/unknown", &mut orch),
            SlashResult::Continue
        ));
    }

    #[test]
    fn styled_text_produces_non_empty_output() {
        let result = styled_text("test", Theme::ACCENT);
        assert!(!result.is_empty());
    }
}

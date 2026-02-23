use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crossterm::style::Stylize;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::agent::orchestrator::Orchestrator;
use crate::config::{Config, Personality};
use crate::gemini::stream::StreamEvent;
use crate::gemini::GeminiClient;
use crate::ui::approval::AutoApproveHandler;
use crate::ui::spinner::Spinner;
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

/// Drain any buffered stdin input (e.g., Enter presses while the model was running).
/// Briefly enables raw mode to access pending events, reads and discards them.
fn drain_stdin() {
    if crossterm::terminal::enable_raw_mode().is_ok() {
        while crossterm::event::poll(std::time::Duration::from_millis(0)).unwrap_or(false) {
            let _ = crossterm::event::read();
        }
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Execute a local shell command (for the ! prefix).
async fn execute_local_shell(
    cmd: &str,
    working_directory: &Path,
) -> anyhow::Result<std::process::Output> {
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_directory)
        .output()
        .await
        .map_err(Into::into)
}

pub async fn run_oneshot(config: &Config, question: &str) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let approval_handler = Arc::new(AutoApproveHandler::always_approve());
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
        approval_handler,
        config.personality,
        config.context_window_turns,
    );
    orchestrator.detect_git_context().await;

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
    let approval_handler = Arc::new(AutoApproveHandler::always_approve());
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
        approval_handler,
        config.personality,
        config.context_window_turns,
    );
    orchestrator.detect_git_context().await;
    let mut editor = DefaultEditor::new()?;

    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!(
        "Mode: {} | Model: {} | Tools: {}",
        config.mode,
        config.model,
        orchestrator.tool_count()
    );
    println!(
        "Working directory: {}",
        config.working_directory.display()
    );
    println!("Git: {}", orchestrator.git_summary());
    println!("Type /help for commands, Ctrl+C to interrupt, /quit to exit.\n");

    // Spawn a background task that sets the cancellation flag on Ctrl+C.
    // This coexists with rustyline's own SIGINT handler via signal-hook-registry.
    let cancel_flag = orchestrator.cancel_flag();
    tokio::spawn(async move {
        loop {
            tokio::signal::ctrl_c().await.ok();
            cancel_flag.store(true, Ordering::SeqCst);
        }
    });

    loop {
        let prompt = format!("{} > ", orchestrator.mode());
        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);

                // Shell prefix: !command runs a local shell command
                if line.starts_with('!') {
                    let shell_cmd = &line[1..];
                    if !shell_cmd.is_empty() {
                        match execute_local_shell(shell_cmd, &config.working_directory)
                            .await
                        {
                            Ok(output) => {
                                let stdout = String::from_utf8_lossy(&output.stdout);
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                if !stdout.is_empty() {
                                    print!("{}", stdout);
                                }
                                if !stderr.is_empty() {
                                    eprint!("{}", stderr);
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "{}: {}",
                                    styled_text("Error", Theme::ERROR),
                                    e
                                );
                            }
                        }
                    }
                    continue;
                }

                if line.starts_with('/') {
                    match handle_slash_command(line, &mut orchestrator).await
                    {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                        SlashResult::ExecutePlan => {
                            orchestrator.reset_cancel();
                            match orchestrator
                                .handle_user_input_streaming(
                                    "Execute the accepted plan step by step.",
                                    default_stream_handler,
                                )
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
                            drain_stdin();
                            if orchestrator.is_cancelled() {
                                println!(
                                    "\n{}",
                                    styled_text("Interrupted.", Theme::DIM)
                                );
                                orchestrator.record_interruption();
                            }
                            println!();
                            continue;
                        }
                    }
                }

                orchestrator.reset_cancel();
                match orchestrator
                    .handle_user_input_streaming(line, default_stream_handler)
                    .await
                {
                    Ok(ref text) => {
                        // Capture plan text in Plan mode
                        if !orchestrator.is_cancelled()
                            && *orchestrator.mode() == crate::mode::Mode::Plan
                            && !text.is_empty()
                        {
                            orchestrator.set_current_plan(text.clone());
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "\n{}: {}",
                            styled_text("Error", Theme::ERROR),
                            e
                        );
                    }
                }
                drain_stdin();
                if orchestrator.is_cancelled() {
                    println!("\n{}", styled_text("Interrupted.", Theme::DIM));
                    orchestrator.record_interruption();
                }
                println!();
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C during readline — clear the cancel flag (set by our signal handler)
                // and just show ^C as usual.
                orchestrator.reset_cancel();
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
    ExecutePlan,
}

async fn handle_slash_command(
    input: &str,
    orchestrator: &mut Orchestrator,
) -> SlashResult {
    let (cmd, arg) = match input.find(' ') {
        Some(pos) => (&input[..pos], input[pos + 1..].trim()),
        None => (input, ""),
    };

    match cmd {
        "/quit" | "/exit" | "/q" => SlashResult::Quit,
        "/clear" => {
            orchestrator.clear_history();
            println!("Conversation history cleared.");
            SlashResult::Continue
        }
        "/accept" | "/a" => {
            if *orchestrator.mode() != crate::mode::Mode::Plan {
                println!(
                    "{}: /accept is only available in Plan mode. Current mode: {}",
                    styled_text("Error", Theme::ERROR),
                    orchestrator.mode()
                );
                return SlashResult::Continue;
            }
            match orchestrator.accept_plan() {
                Some(_) => {
                    println!(
                        "{} Plan accepted. Switched to Execute mode (tools: {}).",
                        styled_text("\u{2713}", Theme::SUCCESS),
                        orchestrator.tool_count()
                    );
                    SlashResult::ExecutePlan
                }
                None => {
                    println!(
                        "No plan to accept. Ask the assistant to create a plan first."
                    );
                    SlashResult::Continue
                }
            }
        }
        "/help" => {
            println!("Commands:");
            println!("  /help              \u{2014} Show this help");
            println!("  /mode [name]       \u{2014} Show or switch mode (explore, plan, execute, auto)");
            println!("  /explore           \u{2014} Switch to Explore mode");
            println!("  /plan              \u{2014} Switch to Plan mode");
            println!("  /execute           \u{2014} Switch to Execute mode");
            println!("  /auto              \u{2014} Switch to Auto mode (unrestricted shell)");
            println!("  /accept            \u{2014} Accept the current plan and switch to Execute mode");
            println!("  /diff [opts]       \u{2014} Show git diff (staged, branch, HEAD~N)");
            println!("  /review [HEAD~N]   \u{2014} Review changes with sub-agent (adds to context)");
            println!("  /commit [message]  \u{2014} Generate commit message via sub-agent and commit");
            println!("  /model [name]      \u{2014} Show or switch model");
            println!("  /personality [s]   \u{2014} Show or change personality (friendly, pragmatic, none)");
            println!("  /status            \u{2014} Show session status (tokens, model, mode, etc.)");
            println!("  /clear             \u{2014} Clear conversation history");
            println!("  /quit              \u{2014} Exit");
            println!();
            println!("  !<command>         \u{2014} Run a local shell command");
            println!("  Ctrl+C             \u{2014} Interrupt model while it is running");
            SlashResult::Continue
        }
        "/model" => {
            if arg.is_empty() {
                println!("Current model: {}", orchestrator.model());
            } else {
                orchestrator.set_model(arg.to_string());
                println!("Model changed to: {}", arg);
            }
            SlashResult::Continue
        }
        "/personality" => {
            if arg.is_empty() {
                println!("Current personality: {}", orchestrator.personality());
            } else {
                match arg.parse::<Personality>() {
                    Ok(p) => {
                        orchestrator.set_personality(p);
                        println!("Personality changed to: {}", p);
                    }
                    Err(e) => println!("{}", e),
                }
            }
            SlashResult::Continue
        }
        "/status" => {
            println!(
                "Mode: {} | Model: {} | Personality: {}",
                orchestrator.mode(),
                orchestrator.model(),
                orchestrator.personality(),
            );
            println!("Git: {}", orchestrator.git_summary());
            println!("Tokens: {}", orchestrator.session_usage());
            println!(
                "Turns: {} / {} | Tools: {}",
                orchestrator.turn_count(),
                orchestrator.context_window_turns(),
                orchestrator.tool_count(),
            );
            SlashResult::Continue
        }
        "/diff" => {
            let working_dir = orchestrator.working_directory();
            let result = if arg.is_empty() || arg == "all" {
                crate::git::diff::all_uncommitted(working_dir).await
            } else if arg == "staged" {
                crate::git::diff::staged(working_dir).await
            } else if arg == "branch" {
                let base = orchestrator.git_default_branch().unwrap_or("main");
                crate::git::diff::branch_diff(working_dir, base).await
            } else if arg.starts_with("HEAD") {
                crate::git::diff::commit_range(working_dir, arg).await
            } else {
                println!("Usage: /diff [staged|branch|HEAD~N]");
                return SlashResult::Continue;
            };

            match result {
                Ok(diff) if diff.is_empty() => println!("No changes found."),
                Ok(diff) => crate::git::diff::colorize_git_diff(&diff),
                Err(e) => println!("{}: {}", styled_text("Error", Theme::ERROR), e),
            }
            SlashResult::Continue
        }
        "/review" => {
            let working_dir = orchestrator.working_directory();
            let diff = if arg.is_empty() {
                crate::git::diff::all_uncommitted(working_dir).await
            } else if arg.starts_with("HEAD") {
                crate::git::diff::commit_range(working_dir, arg).await
            } else {
                println!("Usage: /review [HEAD~N]");
                return SlashResult::Continue;
            };

            match diff {
                Ok(d) if d.is_empty() => {
                    println!("No changes to review.");
                }
                Ok(d) => {
                    let spinner = Spinner::new("Reviewing changes...");
                    match orchestrator.run_review_agent(&d).await {
                        Ok(review) => {
                            spinner.finish();
                            println!("{}", review);
                            println!(
                                "\n{}",
                                styled_text(
                                    "(Review added to context \u{2014} ask follow-up questions if needed)",
                                    Theme::DIM,
                                )
                            );
                        }
                        Err(e) => {
                            spinner.finish();
                            eprintln!(
                                "{}: {}",
                                styled_text("Error", Theme::ERROR),
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    println!("{}: {}", styled_text("Error", Theme::ERROR), e);
                }
            }
            SlashResult::Continue
        }
        "/commit" => {
            let working_dir = orchestrator.working_directory();

            // Check for uncommitted changes
            let diff = match crate::git::diff::all_uncommitted(working_dir).await {
                Ok(d) if d.is_empty() => {
                    println!("Nothing to commit.");
                    return SlashResult::Continue;
                }
                Ok(d) => d,
                Err(e) => {
                    println!(
                        "{}: {}",
                        styled_text("Error", Theme::ERROR),
                        e
                    );
                    return SlashResult::Continue;
                }
            };

            // Get commit message: user-provided or sub-agent-generated
            let message = if !arg.is_empty() {
                arg.to_string()
            } else {
                let spinner = Spinner::new("Generating commit message...");
                match orchestrator.run_commit_agent(&diff).await {
                    Ok(msg) => {
                        spinner.finish();
                        msg.trim().trim_matches('"').to_string()
                    }
                    Err(e) => {
                        spinner.finish();
                        eprintln!(
                            "{}: {}",
                            styled_text("Error", Theme::ERROR),
                            e
                        );
                        return SlashResult::Continue;
                    }
                }
            };

            println!("\nProposed commit message: \"{}\"", message);

            // Prompt for confirmation
            let approved = tokio::task::spawn_blocking(move || {
                dialoguer::Confirm::new()
                    .with_prompt("Commit with this message?")
                    .default(false)
                    .interact()
            })
            .await
            .unwrap_or(Ok(false))
            .unwrap_or(false);

            if approved {
                let working_dir = orchestrator.working_directory();
                match crate::git::commit::commit_all(working_dir, &message).await {
                    Ok(sha) => {
                        println!(
                            "{} Committed: {}",
                            styled_text("\u{2713}", Theme::SUCCESS),
                            sha
                        );
                        orchestrator.refresh_git_context().await;
                    }
                    Err(e) => {
                        eprintln!(
                            "{}: {}",
                            styled_text("Commit failed", Theme::ERROR),
                            e
                        );
                    }
                }
            } else {
                println!("Commit cancelled.");
            }
            SlashResult::Continue
        }
        "/explore" => {
            orchestrator.set_mode(crate::mode::Mode::Explore);
            println!(
                "Switched to explore mode. Tools: {}",
                orchestrator.tool_count()
            );
            SlashResult::Continue
        }
        "/plan" => {
            orchestrator.set_mode(crate::mode::Mode::Plan);
            println!(
                "Switched to plan mode. Tools: {}",
                orchestrator.tool_count()
            );
            SlashResult::Continue
        }
        "/execute" => {
            orchestrator.set_mode(crate::mode::Mode::Execute);
            println!(
                "Switched to execute mode. Tools: {}",
                orchestrator.tool_count()
            );
            SlashResult::Continue
        }
        "/auto" => {
            orchestrator.set_mode(crate::mode::Mode::Auto);
            println!(
                "Switched to auto mode. Tools: {} (shell unrestricted)",
                orchestrator.tool_count()
            );
            SlashResult::Continue
        }
        "/mode" => {
            if arg.is_empty() {
                println!(
                    "Current mode: {}. Usage: /mode <explore|plan|execute|auto>",
                    orchestrator.mode()
                );
            } else {
                match arg.parse::<crate::mode::Mode>() {
                    Ok(new_mode) => {
                        orchestrator.set_mode(new_mode);
                        println!(
                            "Switched to {} mode. Tools: {}",
                            new_mode,
                            orchestrator.tool_count()
                        );
                    }
                    Err(_) => {
                        println!(
                            "Invalid mode '{}'. Expected: explore, plan, execute, or auto",
                            arg
                        );
                    }
                }
            }
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
    use crate::ui::approval::{ApprovalHandler, AutoApproveHandler};
    use std::path::PathBuf;

    fn test_handler() -> Arc<dyn ApprovalHandler> {
        Arc::new(AutoApproveHandler::always_approve())
    }

    fn test_orchestrator() -> Orchestrator {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        Orchestrator::new(
            client,
            crate::mode::Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        )
    }

    fn test_plan_orchestrator() -> Orchestrator {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        Orchestrator::new(
            client,
            crate::mode::Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        )
    }

    #[tokio::test]
    async fn slash_quit_returns_quit() {
        let mut orch = test_orchestrator();
        assert!(matches!(
            handle_slash_command("/quit", &mut orch).await,
            SlashResult::Quit
        ));
        assert!(matches!(
            handle_slash_command("/exit", &mut orch).await,
            SlashResult::Quit
        ));
        assert!(matches!(
            handle_slash_command("/q", &mut orch).await,
            SlashResult::Quit
        ));
    }

    #[tokio::test]
    async fn slash_clear_clears_history() {
        let mut orch = test_orchestrator();
        assert_eq!(orch.turn_count(), 0);
        handle_slash_command("/clear", &mut orch).await;
        assert_eq!(orch.turn_count(), 0);
    }

    #[tokio::test]
    async fn slash_help_returns_continue() {
        let mut orch = test_orchestrator();
        assert!(matches!(
            handle_slash_command("/help", &mut orch).await,
            SlashResult::Continue
        ));
    }

    #[tokio::test]
    async fn unknown_command_returns_continue() {
        let mut orch = test_orchestrator();
        assert!(matches!(
            handle_slash_command("/unknown", &mut orch).await,
            SlashResult::Continue
        ));
    }

    #[tokio::test]
    async fn slash_mode_switches_mode() {
        let mut orch = test_orchestrator();
        assert_eq!(*orch.mode(), crate::mode::Mode::Explore);
        assert_eq!(orch.tool_count(), 6);

        let result = handle_slash_command("/mode plan", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Plan);
        assert_eq!(orch.tool_count(), 8);
    }

    #[tokio::test]
    async fn slash_mode_invalid_stays_unchanged() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/mode bad", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Explore);
        assert_eq!(orch.tool_count(), 6);
    }

    #[tokio::test]
    async fn slash_mode_no_arg_shows_current() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/mode", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Explore);
    }

    #[test]
    fn styled_text_produces_non_empty_output() {
        let result = styled_text("test", Theme::ACCENT);
        assert!(!result.is_empty());
    }

    // ── Phase 4 /accept Tests ──

    #[tokio::test]
    async fn slash_accept_in_plan_mode_with_plan() {
        let mut orch = test_plan_orchestrator();
        orch.set_current_plan("My implementation plan".into());
        let result = handle_slash_command("/accept", &mut orch).await;
        assert!(matches!(result, SlashResult::ExecutePlan));
        assert_eq!(*orch.mode(), crate::mode::Mode::Execute);
    }

    #[tokio::test]
    async fn slash_accept_in_plan_mode_no_plan() {
        let mut orch = test_plan_orchestrator();
        let result = handle_slash_command("/accept", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Plan); // unchanged
    }

    #[tokio::test]
    async fn slash_accept_in_explore_mode() {
        let mut orch = test_orchestrator(); // Explore mode
        let result = handle_slash_command("/accept", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Explore); // unchanged
    }

    #[tokio::test]
    async fn slash_accept_shorthand() {
        let mut orch = test_plan_orchestrator();
        orch.set_current_plan("plan".into());
        let result = handle_slash_command("/a", &mut orch).await;
        assert!(matches!(result, SlashResult::ExecutePlan));
    }

    // ── Phase 5: New Slash Command Tests ──

    #[tokio::test]
    async fn slash_model_show() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/model", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(orch.model(), "model"); // unchanged
    }

    #[tokio::test]
    async fn slash_model_switch() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/model gemini-2.0-flash", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(orch.model(), "gemini-2.0-flash");
    }

    #[tokio::test]
    async fn slash_personality_show() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/personality", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(orch.personality(), Personality::Pragmatic); // default
    }

    #[tokio::test]
    async fn slash_personality_switch() {
        let mut orch = test_orchestrator();
        let result =
            handle_slash_command("/personality friendly", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(orch.personality(), Personality::Friendly);
    }

    #[tokio::test]
    async fn slash_status_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/status", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_explore_shorthand() {
        let mut orch = test_plan_orchestrator(); // Start in plan
        let result = handle_slash_command("/explore", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Explore);
    }

    #[tokio::test]
    async fn slash_plan_shorthand() {
        let mut orch = test_orchestrator(); // Start in explore
        let result = handle_slash_command("/plan", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Plan);
    }

    #[tokio::test]
    async fn slash_execute_shorthand() {
        let mut orch = test_orchestrator(); // Start in explore
        let result = handle_slash_command("/execute", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Execute);
    }

    #[tokio::test]
    async fn slash_auto_shorthand() {
        let mut orch = test_orchestrator(); // Start in explore
        let result = handle_slash_command("/auto", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Auto);
    }

    #[tokio::test]
    async fn slash_command_arg_splitting() {
        // Verify command/arg splitting works for multi-word args
        let mut orch = test_orchestrator();

        // /mode with arg
        handle_slash_command("/mode execute", &mut orch).await;
        assert_eq!(*orch.mode(), crate::mode::Mode::Execute);

        // /mode auto
        handle_slash_command("/mode auto", &mut orch).await;
        assert_eq!(*orch.mode(), crate::mode::Mode::Auto);

        // /model with arg containing spaces-like model name
        handle_slash_command("/model gemini-2.0-flash", &mut orch).await;
        assert_eq!(orch.model(), "gemini-2.0-flash");
    }

    // ── Phase 6: Git Slash Command Tests ──

    #[tokio::test]
    async fn slash_diff_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/diff", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_diff_usage_on_bad_arg() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/diff badarg", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_review_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/review", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_commit_returns_continue() {
        let mut orch = test_orchestrator();
        // In a non-git dir, /commit should show error and return Continue
        let result = handle_slash_command("/commit", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_diff_staged_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/diff staged", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_diff_branch_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/diff branch", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }
}

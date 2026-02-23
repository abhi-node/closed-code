use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crossterm::style::Stylize;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::agent::orchestrator::{Orchestrator, OrchestratorConfig};
use crate::config::{Config, Personality};
use crate::gemini::stream::StreamEvent;
use crate::gemini::types::Part;
use crate::gemini::GeminiClient;
use crate::sandbox::create_sandbox;
use crate::session::store::SessionStore;
use crate::session::transcript::TranscriptWriter;
use crate::session::{SessionEvent, SessionId};
use crate::ui::approval::{ApprovalHandler, DiffOnlyApprovalHandler, TerminalApprovalHandler};
use crate::ui::spinner::Spinner;
use crate::ui::theme::Theme;
use chrono::Utc;

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
    let sandbox = create_sandbox(config.sandbox_mode, config.working_directory.clone());
    let approval_handler: Arc<dyn ApprovalHandler> = Arc::new(DiffOnlyApprovalHandler::new());
    let mut orchestrator = Orchestrator::new(OrchestratorConfig {
        client,
        mode: config.mode,
        working_directory: config.working_directory.clone(),
        max_output_tokens: config.max_output_tokens,
        approval_handler,
        personality: config.personality,
        context_window_turns: config.context_window_turns,
        context_limit_tokens: config.context_limit_tokens,
        sandbox,
        protected_paths: config.protected_paths.clone(),
    });
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
    let sandbox = create_sandbox(config.sandbox_mode, config.working_directory.clone());
    let approval_handler: Arc<dyn ApprovalHandler> = Arc::new(DiffOnlyApprovalHandler::new());
    let mut orchestrator = Orchestrator::new(OrchestratorConfig {
        client,
        mode: config.mode,
        working_directory: config.working_directory.clone(),
        max_output_tokens: config.max_output_tokens,
        approval_handler,
        personality: config.personality,
        context_window_turns: config.context_window_turns,
        context_limit_tokens: config.context_limit_tokens,
        sandbox,
        protected_paths: config.protected_paths.clone(),
    });
    orchestrator.detect_git_context().await;

    // Phase 8a: Auto-start session
    if config.session_auto_save {
        let store = SessionStore::new(config.sessions_dir.clone());
        orchestrator.start_session(store);
    }

    let mut editor = DefaultEditor::new()?;

    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!(
        "Mode: {} | Model: {} | Tools: {}",
        config.mode,
        config.model,
        orchestrator.tool_count()
    );
    println!("Working directory: {}", config.working_directory.display());
    println!("Sandbox: {}", orchestrator.sandbox_summary());
    println!("Git: {}", orchestrator.git_summary());
    if let Some(id) = orchestrator.session_id() {
        println!("Session: {}", &id.as_str()[..8]);
    }
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
                if let Some(shell_cmd) = line.strip_prefix('!') {
                    if !shell_cmd.is_empty() {
                        match execute_local_shell(shell_cmd, &config.working_directory).await {
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
                                eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                            }
                        }
                    }
                    continue;
                }

                if line.starts_with('/') {
                    match handle_slash_command(line, &mut orchestrator).await {
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
                                    eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
                                }
                            }
                            drain_stdin();
                            if orchestrator.is_cancelled() {
                                println!("\n{}", styled_text("Interrupted.", Theme::DIM));
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
                        eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
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

    // Phase 8a: Emit SessionEnd on exit
    orchestrator.emit_event(SessionEvent::SessionEnd {
        timestamp: Utc::now(),
    });

    Ok(())
}

/// Resume a previous session by ID, or list sessions if no ID given.
pub async fn run_resume(config: &Config, session_id_str: Option<&str>) -> anyhow::Result<()> {
    let store = SessionStore::new(config.sessions_dir.clone());

    let session_id = if let Some(id_str) = session_id_str {
        // Try full UUID first, then prefix match
        match SessionId::parse(id_str) {
            Ok(id) => id,
            Err(_) => store
                .find_by_prefix(id_str)
                .map_err(|e| anyhow::anyhow!("{}", e))?,
        }
    } else {
        // Interactive session picker
        let sessions = store
            .list_sessions()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        if sessions.is_empty() {
            println!("No sessions found.");
            return Ok(());
        }

        let items: Vec<String> = sessions
            .iter()
            .map(|meta| {
                format!(
                    "{} ({}) \u{2014} {} \u{2014} {}",
                    &meta.session_id.as_str()[..8],
                    meta.relative_time(),
                    meta.mode,
                    meta.truncated_preview(),
                )
            })
            .collect();

        let selection = tokio::task::spawn_blocking(move || {
            dialoguer::Select::new()
                .with_prompt("Select session to resume")
                .items(&items)
                .default(0)
                .interact_opt()
        })
        .await
        .unwrap_or(Ok(None))
        .unwrap_or(None);

        match selection {
            Some(idx) => sessions[idx].session_id.clone(),
            None => {
                println!("Cancelled.");
                return Ok(());
            }
        }
    };

    let events = store
        .load_events(&session_id)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let history = SessionStore::reconstruct_history(&events);
    let turn_count = history.len();

    println!(
        "Resumed session {} ({} turns restored)",
        &session_id.as_str()[..8],
        turn_count
    );

    // Create orchestrator with restored history
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let sandbox = create_sandbox(config.sandbox_mode, config.working_directory.clone());
    let approval_handler: Arc<dyn ApprovalHandler> = Arc::new(DiffOnlyApprovalHandler::new());
    let mut orchestrator = Orchestrator::new(OrchestratorConfig {
        client,
        mode: config.mode,
        working_directory: config.working_directory.clone(),
        max_output_tokens: config.max_output_tokens,
        approval_handler,
        personality: config.personality,
        context_window_turns: config.context_window_turns,
        context_limit_tokens: config.context_limit_tokens,
        sandbox,
        protected_paths: config.protected_paths.clone(),
    });
    orchestrator.detect_git_context().await;
    orchestrator.set_history(history);
    orchestrator.set_session(session_id, store);

    // Run the same REPL loop
    let mut editor = DefaultEditor::new()?;

    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!(
        "Mode: {} | Model: {} | Tools: {}",
        config.mode,
        config.model,
        orchestrator.tool_count()
    );
    println!("Working directory: {}", config.working_directory.display());
    if let Some(id) = orchestrator.session_id() {
        println!("Session: {} (resumed)", &id.as_str()[..8]);
    }
    println!("Type /help for commands, Ctrl+C to interrupt, /quit to exit.\n");

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

                if let Some(shell_cmd) = line.strip_prefix('!') {
                    if !shell_cmd.is_empty() {
                        match execute_local_shell(shell_cmd, &config.working_directory).await {
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
                                eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                            }
                        }
                    }
                    continue;
                }

                if line.starts_with('/') {
                    match handle_slash_command(line, &mut orchestrator).await {
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
                                    eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
                                }
                            }
                            drain_stdin();
                            if orchestrator.is_cancelled() {
                                println!("\n{}", styled_text("Interrupted.", Theme::DIM));
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
                        if !orchestrator.is_cancelled()
                            && *orchestrator.mode() == crate::mode::Mode::Plan
                            && !text.is_empty()
                        {
                            orchestrator.set_current_plan(text.clone());
                        }
                    }
                    Err(e) => {
                        eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
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

    orchestrator.emit_event(SessionEvent::SessionEnd {
        timestamp: Utc::now(),
    });

    Ok(())
}

enum SlashResult {
    Continue,
    Quit,
    ExecutePlan,
}

/// Return the appropriate ApprovalHandler for a given mode.
/// Guided mode uses interactive terminal approval; all others use diff-only auto-approve.
fn handler_for_mode(mode: &crate::mode::Mode) -> Arc<dyn ApprovalHandler> {
    match mode {
        crate::mode::Mode::Guided => Arc::new(TerminalApprovalHandler::new()),
        _ => Arc::new(DiffOnlyApprovalHandler::new()),
    }
}

async fn handle_slash_command(input: &str, orchestrator: &mut Orchestrator) -> SlashResult {
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

            if orchestrator.current_plan().is_none() {
                println!("No plan to accept. Ask the assistant to create a plan first.");
                return SlashResult::Continue;
            }

            // Prompt user to choose target mode
            let items = vec![
                "Guided  \u{2014} Write files with approval for each change",
                "Execute \u{2014} Auto-approve writes, allowlisted shell",
                "Auto    \u{2014} Full autonomy, unrestricted shell [DANGER]",
            ];

            let selection = tokio::task::spawn_blocking(move || {
                dialoguer::Select::new()
                    .with_prompt("Choose execution mode")
                    .items(&items)
                    .default(0)
                    .interact_opt()
            })
            .await
            .unwrap_or(Ok(None))
            .unwrap_or(None);

            let target_mode = match selection {
                Some(0) => crate::mode::Mode::Guided,
                Some(1) => crate::mode::Mode::Execute,
                Some(2) => {
                    // Auto mode danger warning
                    println!(
                        "{}",
                        styled_text(
                            "WARNING: Auto mode grants unrestricted shell access and auto-approves all changes.",
                            Theme::ERROR,
                        )
                    );
                    let confirmed = tokio::task::spawn_blocking(|| {
                        dialoguer::Confirm::new()
                            .with_prompt("Are you sure?")
                            .default(false)
                            .interact()
                    })
                    .await
                    .unwrap_or(Ok(false))
                    .unwrap_or(false);

                    if !confirmed {
                        println!("Cancelled. Staying in Plan mode.");
                        return SlashResult::Continue;
                    }
                    crate::mode::Mode::Auto
                }
                _ => {
                    println!("Cancelled.");
                    return SlashResult::Continue;
                }
            };

            // Swap handler and accept the plan
            let handler = handler_for_mode(&target_mode);
            orchestrator.set_mode_with_handler(target_mode, Some(handler));
            match orchestrator.accept_plan(target_mode) {
                Some(_) => {
                    let extra = if target_mode == crate::mode::Mode::Guided {
                        " Each change requires your approval."
                    } else {
                        ""
                    };
                    println!(
                        "{} Plan accepted. Switched to {} mode (tools: {}).{}",
                        styled_text("\u{2713}", Theme::SUCCESS),
                        target_mode,
                        orchestrator.tool_count(),
                        extra,
                    );
                    SlashResult::ExecutePlan
                }
                None => {
                    println!("No plan to accept.");
                    SlashResult::Continue
                }
            }
        }
        "/help" => {
            println!("Commands:");
            println!("  /help              \u{2014} Show this help");
            println!("  /mode [name]       \u{2014} Show or switch mode (explore, plan, guided, execute, auto)");
            println!("  /explore           \u{2014} Switch to Explore mode");
            println!("  /plan              \u{2014} Switch to Plan mode");
            println!(
                "  /guided            \u{2014} Switch to Guided mode (writes require approval)"
            );
            println!("  /execute           \u{2014} Switch to Execute mode");
            println!("  /auto              \u{2014} Switch to Auto mode (unrestricted shell)");
            println!("  /accept            \u{2014} Accept plan and choose execution mode");
            println!("  /diff [opts]       \u{2014} Show git diff (staged, branch, HEAD~N)");
            println!(
                "  /review [HEAD~N]   \u{2014} Review changes with sub-agent (adds to context)"
            );
            println!(
                "  /commit [message]  \u{2014} Generate commit message via sub-agent and commit"
            );
            println!("  /model [name]      \u{2014} Show or switch model");
            println!("  /personality [s]   \u{2014} Show or change personality (friendly, pragmatic, none)");
            println!(
                "  /sandbox           \u{2014} Show sandbox mode, backend, and protected paths"
            );
            println!(
                "  /status            \u{2014} Show session status (tokens, model, mode, etc.)"
            );
            println!("  /new               \u{2014} Start a new session (clears history)");
            println!("  /fork              \u{2014} Fork current session into a new one");
            println!(
                "  /compact [prompt]  \u{2014} Compact conversation history via LLM summarization"
            );
            println!("  /history [N]       \u{2014} Show last N conversation turns (default: 10)");
            println!("  /export [file]     \u{2014} Export session transcript to markdown");
            println!("  /resume            \u{2014} List recent sessions (use `closed-code resume` to resume)");
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
            println!("Sandbox: {}", orchestrator.sandbox_summary());
            println!("Git: {}", orchestrator.git_summary());
            println!("Usage: {}", orchestrator.session_usage());
            let prompt_tokens = orchestrator.last_prompt_tokens();
            if prompt_tokens > 0 {
                println!(
                    "Context: {} / {} tokens",
                    prompt_tokens,
                    orchestrator.context_limit_tokens()
                );
            } else {
                println!(
                    "Context: {} / {} turns (no token data yet)",
                    orchestrator.turn_count(),
                    orchestrator.context_window_turns()
                );
            }
            println!(
                "Turns: {} | Tools: {}",
                orchestrator.turn_count(),
                orchestrator.tool_count()
            );
            if let Some(id) = orchestrator.session_id() {
                println!("Session: {} (auto-save enabled)", &id.as_str()[..8]);
            }
            SlashResult::Continue
        }
        "/sandbox" => {
            println!("Sandbox mode: {}", orchestrator.sandbox_mode());
            println!("Summary: {}", orchestrator.sandbox_summary());
            println!("Protected paths (always):");
            println!("  .git, .closed-code, .env, *.pem, *.key");
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
                            eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
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
                    println!("{}: {}", styled_text("Error", Theme::ERROR), e);
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
                        eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
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
                        eprintln!("{}: {}", styled_text("Commit failed", Theme::ERROR), e);
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
        "/guided" => {
            let handler = handler_for_mode(&crate::mode::Mode::Guided);
            orchestrator.set_mode_with_handler(crate::mode::Mode::Guided, Some(handler));
            println!(
                "Switched to guided mode. Tools: {}. File changes require approval.",
                orchestrator.tool_count()
            );
            SlashResult::Continue
        }
        "/execute" => {
            let handler = handler_for_mode(&crate::mode::Mode::Execute);
            orchestrator.set_mode_with_handler(crate::mode::Mode::Execute, Some(handler));
            println!(
                "Switched to execute mode. Tools: {}",
                orchestrator.tool_count()
            );
            SlashResult::Continue
        }
        "/auto" => {
            let handler = handler_for_mode(&crate::mode::Mode::Auto);
            orchestrator.set_mode_with_handler(crate::mode::Mode::Auto, Some(handler));
            println!(
                "Switched to auto mode. Tools: {} (shell unrestricted)",
                orchestrator.tool_count()
            );
            SlashResult::Continue
        }
        "/mode" => {
            if arg.is_empty() {
                println!(
                    "Current mode: {}. Usage: /mode <explore|plan|guided|execute|auto>",
                    orchestrator.mode()
                );
            } else {
                match arg.parse::<crate::mode::Mode>() {
                    Ok(new_mode) => {
                        let handler = handler_for_mode(&new_mode);
                        orchestrator.set_mode_with_handler(new_mode, Some(handler));
                        println!(
                            "Switched to {} mode. Tools: {}",
                            new_mode,
                            orchestrator.tool_count()
                        );
                    }
                    Err(_) => {
                        println!(
                            "Invalid mode '{}'. Expected: explore, plan, guided, execute, or auto",
                            arg
                        );
                    }
                }
            }
            SlashResult::Continue
        }
        // ── Phase 8a: Session Commands ──
        "/new" => {
            orchestrator.clear_history();
            if let Some(id) = orchestrator.session_id() {
                println!("New session started: {}", &id.as_str()[..8]);
            } else {
                println!("History cleared. (No session store configured.)");
            }
            SlashResult::Continue
        }
        "/fork" => {
            match orchestrator.fork_session() {
                Ok(Some(new_id)) => {
                    println!(
                        "{} Forked to new session: {}",
                        styled_text("\u{2713}", Theme::SUCCESS),
                        &new_id.as_str()[..8]
                    );
                }
                Ok(None) => {
                    println!("No active session to fork.");
                }
                Err(e) => {
                    eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                }
            }
            SlashResult::Continue
        }
        "/compact" => {
            let user_prompt = if arg.is_empty() { None } else { Some(arg) };
            let turns_before = orchestrator.turn_count();
            let spinner = Spinner::new("Compacting...");
            match orchestrator.compact_history(user_prompt).await {
                Ok(summary) => {
                    spinner.finish();
                    println!(
                        "{} Compacted: {} turns \u{2192} {} turns",
                        styled_text("\u{2713}", Theme::SUCCESS),
                        turns_before,
                        orchestrator.turn_count(),
                    );
                    println!("{}", styled_text("Summary:", Theme::DIM),);
                    // Show first 200 chars of summary
                    let preview = if summary.len() > 200 {
                        format!("{}...", &summary[..197])
                    } else {
                        summary
                    };
                    println!("{}", preview);
                }
                Err(e) => {
                    spinner.finish();
                    eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                }
            }
            SlashResult::Continue
        }
        "/history" => {
            let n: usize = if arg.is_empty() {
                10
            } else {
                arg.parse().unwrap_or(10)
            };

            let history = orchestrator.history();
            if history.is_empty() {
                println!("No conversation history.");
            } else {
                let start = history.len().saturating_sub(n);
                for (i, content) in history[start..].iter().enumerate() {
                    let role = content.role.as_deref().unwrap_or("system");
                    let text: String = content
                        .parts
                        .iter()
                        .map(|p| match p {
                            Part::Text(t) => t.as_str(),
                            Part::FunctionCall { name, .. } => name.as_str(),
                            Part::FunctionResponse { name, .. } => name.as_str(),
                            Part::InlineData { mime_type, .. } => mime_type.as_str(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let truncated = if text.len() > 120 {
                        format!("{}...", &text[..117])
                    } else {
                        text
                    };
                    println!("  [{}] {}: {}", start + i + 1, role, truncated);
                }
            }
            SlashResult::Continue
        }
        "/export" => {
            let file_path = if arg.is_empty() { "transcript.md" } else { arg };

            if let Some(store) = orchestrator.session_store() {
                if let Some(id) = orchestrator.session_id() {
                    match store.load_events(id) {
                        Ok(events) => match TranscriptWriter::write_to_file(&events, file_path) {
                            Ok(()) => {
                                println!(
                                    "{} Exported {} events to {}",
                                    styled_text("\u{2713}", Theme::SUCCESS),
                                    events.len(),
                                    file_path
                                );
                            }
                            Err(e) => {
                                eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                            }
                        },
                        Err(e) => {
                            eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                        }
                    }
                } else {
                    println!("No active session to export.");
                }
            } else {
                println!("No session store configured. Enable [session] auto_save = true.");
            }
            SlashResult::Continue
        }
        "/resume" => {
            if let Some(store) = orchestrator.session_store().cloned() {
                match store.list_sessions() {
                    Ok(sessions) if sessions.is_empty() => {
                        println!("No sessions found.");
                    }
                    Ok(sessions) => {
                        // Find current session index for default selection
                        let current_idx = orchestrator
                            .session_id()
                            .and_then(|current| {
                                sessions.iter().position(|m| m.session_id == *current)
                            })
                            .unwrap_or(0);

                        let current_session_id = orchestrator.session_id().cloned();
                        let items: Vec<String> = sessions
                            .iter()
                            .map(|meta| {
                                let marker =
                                    if Some(&meta.session_id) == current_session_id.as_ref() {
                                        " (current)"
                                    } else {
                                        ""
                                    };
                                format!(
                                    "{} ({}) \u{2014} {} \u{2014} {}{}",
                                    &meta.session_id.as_str()[..8],
                                    meta.relative_time(),
                                    meta.mode,
                                    meta.truncated_preview(),
                                    marker,
                                )
                            })
                            .collect();

                        let selection = tokio::task::spawn_blocking(move || {
                            dialoguer::Select::new()
                                .with_prompt("Select session to resume")
                                .items(&items)
                                .default(current_idx)
                                .interact_opt()
                        })
                        .await
                        .unwrap_or(Ok(None))
                        .unwrap_or(None);

                        if let Some(idx) = selection {
                            let selected = &sessions[idx];
                            if Some(&selected.session_id) == current_session_id.as_ref() {
                                println!("Already on this session.");
                            } else {
                                match store.load_events(&selected.session_id) {
                                    Ok(events) => {
                                        let history = SessionStore::reconstruct_history(&events);
                                        let turn_count = history.len();
                                        orchestrator.set_history(history);
                                        orchestrator.set_session(
                                            selected.session_id.clone(),
                                            store.clone(),
                                        );
                                        println!(
                                            "Switched to session {} ({} turns restored)",
                                            &selected.session_id.as_str()[..8],
                                            turn_count
                                        );
                                    }
                                    Err(e) => {
                                        eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e);
                    }
                }
            } else {
                println!("No session store configured.");
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
    use crate::gemini::types::Content;
    use crate::sandbox::mock::MockSandbox;
    use crate::sandbox::Sandbox;
    use crate::ui::approval::{ApprovalHandler, AutoApproveHandler};
    use std::path::PathBuf;

    fn test_handler() -> Arc<dyn ApprovalHandler> {
        Arc::new(AutoApproveHandler::always_approve())
    }

    fn mock_sandbox() -> Arc<dyn Sandbox> {
        Arc::new(MockSandbox::new(PathBuf::from("/tmp")))
    }

    fn test_orchestrator() -> Orchestrator {
        Orchestrator::new(OrchestratorConfig {
            client: Arc::new(GeminiClient::new("key".into(), "model".into())),
            mode: crate::mode::Mode::Explore,
            working_directory: PathBuf::from("/tmp"),
            max_output_tokens: 8192,
            approval_handler: test_handler(),
            personality: Personality::default(),
            context_window_turns: 50,
            context_limit_tokens: 1_000_000,
            sandbox: mock_sandbox(),
            protected_paths: vec![],
        })
    }

    fn test_plan_orchestrator() -> Orchestrator {
        Orchestrator::new(OrchestratorConfig {
            client: Arc::new(GeminiClient::new("key".into(), "model".into())),
            mode: crate::mode::Mode::Plan,
            working_directory: PathBuf::from("/tmp"),
            max_output_tokens: 8192,
            approval_handler: test_handler(),
            personality: Personality::default(),
            context_window_turns: 50,
            context_limit_tokens: 1_000_000,
            sandbox: mock_sandbox(),
            protected_paths: vec![],
        })
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
    // Note: /accept with a plan now uses dialoguer::Select which requires stdin,
    // so we test the orchestrator's accept_plan(mode) directly instead.

    #[test]
    fn accept_plan_via_orchestrator_execute() {
        let mut orch = test_plan_orchestrator();
        orch.set_current_plan("My implementation plan".into());
        let plan = orch.accept_plan(crate::mode::Mode::Execute);
        assert!(plan.is_some());
        assert_eq!(*orch.mode(), crate::mode::Mode::Execute);
    }

    #[test]
    fn accept_plan_via_orchestrator_guided() {
        let mut orch = test_plan_orchestrator();
        orch.set_current_plan("My plan".into());
        let plan = orch.accept_plan(crate::mode::Mode::Guided);
        assert!(plan.is_some());
        assert_eq!(*orch.mode(), crate::mode::Mode::Guided);
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
        let result = handle_slash_command("/personality friendly", &mut orch).await;
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
    async fn slash_guided_shorthand() {
        let mut orch = test_orchestrator(); // Start in explore
        let result = handle_slash_command("/guided", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Guided);
        assert_eq!(orch.tool_count(), 9); // write + spawn_planner tools registered
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

        // /mode guided
        handle_slash_command("/mode guided", &mut orch).await;
        assert_eq!(*orch.mode(), crate::mode::Mode::Guided);

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

    // ── Phase 7: Sandbox Slash Command Tests ──

    #[tokio::test]
    async fn slash_sandbox_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/sandbox", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[test]
    fn test_orchestrator_sandbox_mode() {
        let orch = test_orchestrator();
        // MockSandbox defaults to FullAccess
        assert_eq!(orch.sandbox_mode(), crate::sandbox::SandboxMode::FullAccess);
    }

    #[test]
    fn test_orchestrator_sandbox_summary_non_empty() {
        let orch = test_orchestrator();
        let summary = orch.sandbox_summary();
        assert!(!summary.is_empty());
    }

    // ── Phase 8a: Session Slash Command Tests ──

    #[tokio::test]
    async fn slash_new_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/new", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(orch.turn_count(), 0);
    }

    #[tokio::test]
    async fn slash_fork_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/fork", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_fork_with_session() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        orch.start_session(store);

        let original_id = orch.session_id().unwrap().clone();
        let result = handle_slash_command("/fork", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        // Session ID should have changed
        assert_ne!(orch.session_id().unwrap(), &original_id);
    }

    #[tokio::test]
    async fn slash_compact_too_short() {
        let mut orch = test_orchestrator();
        // No history — should error
        let result = handle_slash_command("/compact", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_history_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/history", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_history_with_turns() {
        let mut orch = test_orchestrator();
        // Manually add some history through set_history
        orch.set_history(vec![Content::user("hello"), Content::model("hi")]);
        let result = handle_slash_command("/history 5", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_export_no_session() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/export", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_export_with_session() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        orch.start_session(store);

        let export_path = dir.path().join("test_transcript.md");
        let cmd = format!("/export {}", export_path.display());
        let result = handle_slash_command(&cmd, &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert!(export_path.exists());
    }

    #[tokio::test]
    async fn slash_resume_returns_continue() {
        let mut orch = test_orchestrator();
        let result = handle_slash_command("/resume", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_new_with_session_starts_new() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        orch.start_session(store);

        let original_id = orch.session_id().unwrap().clone();
        orch.set_history(vec![Content::user("hello")]);

        let result = handle_slash_command("/new", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(orch.turn_count(), 0);
        assert_ne!(orch.session_id().unwrap(), &original_id);
    }

    #[tokio::test]
    async fn slash_help_includes_session_commands() {
        let mut orch = test_orchestrator();
        // Just verify it returns Continue (help output goes to stdout)
        let result = handle_slash_command("/help", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }

    #[tokio::test]
    async fn slash_status_with_session() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        orch.start_session(store);

        let result = handle_slash_command("/status", &mut orch).await;
        assert!(matches!(result, SlashResult::Continue));
    }
}

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::agent::orchestrator::Orchestrator;
use crate::config::Personality;
use crate::mode::Mode;

use super::chat::ChatMessage;
use super::command_picker::all_commands;
use super::events::AppEvent;
use super::message::SystemSeverity;

/// Result of dispatching a slash command.
#[derive(Debug)]
pub enum CommandResult {
    /// Command processed, continue normal operation.
    Continue,
    /// User requested quit.
    Quit,
    /// Plan was accepted — send it to the LLM.
    ExecutePlan,
    /// Switch mode (for /accept which needs special handling).
    SwitchMode(Mode),
    /// Show the session picker overlay (Phase 9d).
    ShowSessionPicker,
    /// Show the mode picker overlay (Phase 9d).
    ShowModePicker,
    /// Run commit agent asynchronously (no args provided).
    RunCommitAgent { diff: String, working_dir: PathBuf },
    /// Run review agent asynchronously.
    RunReviewAgent { diff: String, working_dir: PathBuf },
    /// Run compact asynchronously.
    RunCompact { user_prompt: Option<String>, turns_before: usize },
    /// Reindex file fuzzy search.
    Reindex,
}

/// Dispatch a slash command and return messages to display + result.
pub async fn dispatch(
    input: &str,
    orchestrator: &mut Orchestrator,
    event_tx: Option<&mpsc::UnboundedSender<AppEvent>>,
) -> (Vec<ChatMessage>, CommandResult) {
    let (cmd, arg) = parse_command(input);
    let mut messages = Vec::new();

    let result = match cmd {
        "/quit" | "/exit" | "/q" => CommandResult::Quit,

        "/clear" => {
            orchestrator.clear_history();
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: "Conversation history cleared.".into(),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/help" => {
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format_help_text(),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/mode" => {
            if arg.is_empty() {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Info,
                    text: format!(
                        "Current mode: {}. Usage: /mode <explore|plan|guided|execute|auto>",
                        orchestrator.mode()
                    ),
                    diff_lines: None,
                });
            } else {
                match arg.parse::<Mode>() {
                    Ok(new_mode) => {
                        switch_mode(orchestrator, new_mode, event_tx);
                        messages.push(ChatMessage::System {
                            severity: SystemSeverity::Info,
                            text: format!(
                                "Switched to {} mode. Tools: {}",
                                new_mode,
                                orchestrator.tool_count()
                            ),
                            diff_lines: None,
                        });
                    }
                    Err(_) => {
                        messages.push(ChatMessage::System {
                            severity: SystemSeverity::Error,
                            text: format!(
                                "Invalid mode '{}'. Expected: explore, plan, guided, execute, or auto",
                                arg
                            ),
                            diff_lines: None,
                        });
                    }
                }
            }
            CommandResult::Continue
        }

        "/explore" => {
            switch_mode(orchestrator, Mode::Explore, event_tx);
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format!(
                    "Switched to Explore mode. Tools: {}",
                    orchestrator.tool_count()
                ),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/plan" => {
            switch_mode(orchestrator, Mode::Plan, event_tx);
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format!(
                    "Switched to Plan mode. Tools: {}",
                    orchestrator.tool_count()
                ),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/guided" => {
            switch_mode(orchestrator, Mode::Guided, event_tx);
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format!(
                    "Switched to Guided mode. Tools: {}. File changes require approval.",
                    orchestrator.tool_count()
                ),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/execute" => {
            switch_mode(orchestrator, Mode::Execute, event_tx);
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format!(
                    "Switched to Execute mode. Tools: {}",
                    orchestrator.tool_count()
                ),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/auto" => {
            switch_mode(orchestrator, Mode::Auto, event_tx);
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format!(
                    "Switched to Auto mode. Tools: {} (shell unrestricted)",
                    orchestrator.tool_count()
                ),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/accept" | "/a" => {
            if *orchestrator.mode() != Mode::Plan {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Error,
                    text: format!(
                        "Error: /accept is only available in Plan mode. Current mode: {}",
                        orchestrator.mode()
                    ),
                    diff_lines: None,
                });
                return (messages, CommandResult::Continue);
            }

            if orchestrator.current_plan_text().is_none() {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Error,
                    text: "No plan to accept. Ask the assistant to create a plan first.".into(),
                    diff_lines: None,
                });
                return (messages, CommandResult::Continue);
            }

            if orchestrator.is_plan_accepted() {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Error,
                    text: "Plan has already been accepted. Generate a new plan to accept again.".into(),
                    diff_lines: None,
                });
                return (messages, CommandResult::Continue);
            }

            // In TUI, show mode picker overlay (Phase 9d)
            // For now, default to Guided mode
            CommandResult::ShowModePicker
        }

        "/diff" => {
            let working_dir = orchestrator.working_directory().to_path_buf();
            let result = if arg.is_empty() || arg == "all" {
                crate::git::diff::all_uncommitted(&working_dir).await
            } else if arg == "staged" {
                crate::git::diff::staged(&working_dir).await
            } else if arg == "branch" {
                let base = orchestrator
                    .git_default_branch()
                    .unwrap_or("main")
                    .to_string();
                crate::git::diff::branch_diff(&working_dir, &base).await
            } else if arg.starts_with("HEAD") {
                crate::git::diff::commit_range(&working_dir, arg).await
            } else {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Error,
                    text: "Usage: /diff [staged|branch|HEAD~N]".into(),
                    diff_lines: None,
                });
                return (messages, CommandResult::Continue);
            };

            match result {
                Ok(diff) if diff.is_empty() => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Info,
                        text: "No changes found.".into(),
                        diff_lines: None,
                    });
                }
                Ok(diff) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Info,
                        text: format!("```diff\n{}\n```", diff),
                        diff_lines: None,
                    });
                }
                Err(e) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Error,
                        text: format!("Error: {}", e),
                        diff_lines: None,
                    });
                }
            }
            CommandResult::Continue
        }

        "/review" => {
            let working_dir = orchestrator.working_directory().to_path_buf();
            let diff = if arg.is_empty() {
                crate::git::diff::all_uncommitted(&working_dir).await
            } else if arg.starts_with("HEAD") {
                crate::git::diff::commit_range(&working_dir, arg).await
            } else {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Error,
                    text: "Usage: /review [HEAD~N]".into(),
                    diff_lines: None,
                });
                return (messages, CommandResult::Continue);
            };

            match diff {
                Ok(d) if d.is_empty() => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Info,
                        text: "No changes to review.".into(),
                        diff_lines: None,
                    });
                    CommandResult::Continue
                }
                Ok(d) => CommandResult::RunReviewAgent {
                    diff: d,
                    working_dir,
                },
                Err(e) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Error,
                        text: format!("Error: {}", e),
                        diff_lines: None,
                    });
                    CommandResult::Continue
                }
            }
        }

        "/commit" => {
            let working_dir = orchestrator.working_directory().to_path_buf();

            let diff = match crate::git::diff::all_uncommitted(&working_dir).await {
                Ok(d) if d.is_empty() => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Info,
                        text: "Nothing to commit.".into(),
                        diff_lines: None,
                    });
                    return (messages, CommandResult::Continue);
                }
                Ok(d) => d,
                Err(e) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Error,
                        text: format!("Error: {}", e),
                        diff_lines: None,
                    });
                    return (messages, CommandResult::Continue);
                }
            };

            if !arg.is_empty() {
                // User provided commit message — commit directly (fast, no agent)
                match crate::git::commit::commit_all(&working_dir, arg).await {
                    Ok(sha) => {
                        orchestrator.refresh_git_context().await;
                        messages.push(ChatMessage::System {
                            severity: SystemSeverity::Success,
                            text: format!("Committed: {} ({})", sha, arg),
                            diff_lines: None,
                        });
                    }
                    Err(e) => {
                        messages.push(ChatMessage::System {
                            severity: SystemSeverity::Error,
                            text: format!("Commit failed: {}", e),
                            diff_lines: None,
                        });
                    }
                }
                CommandResult::Continue
            } else {
                // No message — need commit agent (async)
                CommandResult::RunCommitAgent { diff, working_dir }
            }
        }

        "/model" => {
            if arg.is_empty() {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Info,
                    text: format!("Current model: {}", orchestrator.model()),
                    diff_lines: None,
                });
            } else {
                orchestrator.set_model(arg.to_string());
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Info,
                    text: format!("Model changed to: {}", arg),
                    diff_lines: None,
                });
            }
            CommandResult::Continue
        }

        "/personality" => {
            if arg.is_empty() {
                messages.push(ChatMessage::System {
                    severity: SystemSeverity::Info,
                    text: format!("Current personality: {}", orchestrator.personality()),
                    diff_lines: None,
                });
            } else {
                match arg.parse::<Personality>() {
                    Ok(p) => {
                        orchestrator.set_personality(p);
                        messages.push(ChatMessage::System {
                            severity: SystemSeverity::Info,
                            text: format!("Personality changed to: {}", p),
                            diff_lines: None,
                        });
                    }
                    Err(e) => {
                        messages.push(ChatMessage::System {
                            severity: SystemSeverity::Error,
                            text: e.to_string(),
                            diff_lines: None,
                        });
                    }
                }
            }
            CommandResult::Continue
        }

        "/status" => {
            let prompt_tokens = orchestrator.last_prompt_tokens();
            let context_info = if prompt_tokens > 0 {
                format!(
                    "Context: {} / {} tokens",
                    prompt_tokens,
                    orchestrator.context_limit_tokens()
                )
            } else {
                "Context: no token data yet".to_string()
            };
            let mut status_text = format!(
                "Mode: {} | Model: {} | Personality: {}\n\
                 Sandbox: {}\n\
                 Git: {}\n\
                 Usage: {}\n\
                 {}\n\
                 Turns: {} | Tools: {}",
                orchestrator.mode(),
                orchestrator.model(),
                orchestrator.personality(),
                orchestrator.sandbox_summary(),
                orchestrator.git_summary(),
                orchestrator.session_usage(),
                context_info,
                orchestrator.turn_count(),
                orchestrator.tool_count(),
            );
            if let Some(id) = orchestrator.session_id() {
                status_text.push_str(&format!(
                    "\nSession: {} (auto-save enabled)",
                    &id.as_str()[..8]
                ));
            }
            messages.push(ChatMessage::System { severity: SystemSeverity::Info, text: status_text, diff_lines: None });
            CommandResult::Continue
        }

        "/sandbox" => {
            let protected = orchestrator.protected_paths();
            let protected_display = if protected.is_empty() {
                "  (none configured)".to_string()
            } else {
                protected
                    .iter()
                    .map(|p| format!("  {}", p))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format!(
                    "Sandbox mode: {}\nSummary: {}\nProtected paths:\n  .git, .closed-code, .env, *.pem, *.key\n{}",
                    orchestrator.sandbox_mode(),
                    orchestrator.sandbox_summary(),
                    protected_display,
                ),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/new" => {
            orchestrator.clear_history();
            let msg = if let Some(id) = orchestrator.session_id() {
                format!("New session started: {}", &id.as_str()[..8])
            } else {
                "History cleared. (No session store configured.)".into()
            };
            messages.push(ChatMessage::System { severity: SystemSeverity::Info, text: msg, diff_lines: None });
            CommandResult::Continue
        }

        "/fork" => {
            match orchestrator.fork_session() {
                Ok(Some(new_id)) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Success,
                        text: format!("Forked to new session: {}", &new_id.as_str()[..8]),
                        diff_lines: None,
                    });
                }
                Ok(None) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Info,
                        text: "No active session to fork.".into(),
                        diff_lines: None,
                    });
                }
                Err(e) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Error,
                        text: format!("Error: {}", e),
                        diff_lines: None,
                    });
                }
            }
            CommandResult::Continue
        }

        "/compact" => {
            let user_prompt = if arg.is_empty() { None } else { Some(arg.to_string()) };
            let turns_before = orchestrator.turn_count();
            CommandResult::RunCompact { user_prompt, turns_before }
        }

        "/history" => {
            let n: usize = if arg.is_empty() {
                10
            } else {
                arg.parse().unwrap_or(10)
            };
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: orchestrator.recent_history_display(n),
                diff_lines: None,
            });
            CommandResult::Continue
        }

        "/export" => {
            let file_path = if arg.is_empty() { "transcript.md" } else { arg };
            match orchestrator.export_session(file_path) {
                Ok(()) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Success,
                        text: format!("Exported session to {}", file_path),
                        diff_lines: None,
                    });
                }
                Err(e) => {
                    messages.push(ChatMessage::System {
                        severity: SystemSeverity::Error,
                        text: format!("Error: {}", e),
                        diff_lines: None,
                    });
                }
            }
            CommandResult::Continue
        }

        "/reindex" => {
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Info,
                text: "Rebuilding file index...".into(),
                diff_lines: None,
            });
            CommandResult::Reindex
        }

        "/resume" => {
            // In TUI, show session picker overlay (Phase 9d)
            CommandResult::ShowSessionPicker
        }

        _ => {
            messages.push(ChatMessage::System {
                severity: SystemSeverity::Error,
                text: format!(
                    "Unknown command: {}. Type /help for available commands.",
                    cmd
                ),
                diff_lines: None,
            });
            CommandResult::Continue
        }
    };

    (messages, result)
}

/// Parse input into (command, args).
pub fn parse_command(input: &str) -> (&str, &str) {
    match input.find(' ') {
        Some(pos) => (&input[..pos], input[pos + 1..].trim()),
        None => (input, ""),
    }
}

/// Switch mode with the appropriate approval handler.
///
/// In TUI mode, we always use `TuiApprovalHandler` for modes that support
/// file editing (Guided, Execute, Auto). The TUI event loop handles
/// auto-approval for Execute/Auto modes, which allows diffs to be shown
/// in the chat history.
fn switch_mode(
    orchestrator: &mut Orchestrator,
    mode: Mode,
    event_tx: Option<&mpsc::UnboundedSender<AppEvent>>,
) {
    use crate::ui::approval::{AutoApproveHandler, DiffOnlyApprovalHandler};
    use std::sync::Arc;

    use super::tui_approval_handler::TuiApprovalHandler;

    let handler: Option<Arc<dyn crate::ui::approval::ApprovalHandler>> = match mode {
        Mode::Guided | Mode::Execute | Mode::Auto => {
            if let Some(tx) = event_tx {
                // TUI mode: route all approvals through the TUI event loop.
                // The event loop auto-approves for Execute/Auto modes.
                Some(Arc::new(TuiApprovalHandler::new(tx.clone())))
            } else {
                // CLI/REPL fallback
                if mode == Mode::Guided {
                    Some(Arc::new(DiffOnlyApprovalHandler::new()))
                } else {
                    Some(Arc::new(AutoApproveHandler::always_approve()))
                }
            }
        }
        _ => None,
    };
    orchestrator.set_mode_with_handler(mode, handler);
}

/// Execute a shell command and return output as a ChatMessage.
pub async fn execute_shell_command(command: &str) -> ChatMessage {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut text = String::new();

            if !stdout.is_empty() {
                text.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            if text.is_empty() {
                text.push_str("(no output)");
            }

            // Truncate very long output
            if text.len() > 5000 {
                text.truncate(5000);
                text.push_str("\n... (truncated)");
            }

            let exit_code = out.status.code().unwrap_or(-1);
            ChatMessage::System {
                severity: SystemSeverity::Info,
                text: format!("$ {}\n{}\n[exit {}]", command, text.trim(), exit_code),
                diff_lines: None,
            }
        }
        Err(e) => ChatMessage::System {
            severity: SystemSeverity::Error,
            text: format!("$ {}\nError: {}", command, e),
            diff_lines: None,
        },
    }
}

/// Generate help text from the command registry.
fn format_help_text() -> String {
    let commands = all_commands();
    let mut text = String::from("Commands:\n");
    for cmd in &commands {
        let display = cmd.display_name();
        text.push_str(&format!("  {:<22} {}\n", display, cmd.description));
    }
    text.push_str("\n  !<command>             Run a local shell command\n");
    text.push_str("  Ctrl+C                 Interrupt model\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_no_args() {
        let (cmd, args) = parse_command("/help");
        assert_eq!(cmd, "/help");
        assert_eq!(args, "");
    }

    #[test]
    fn parse_command_with_args() {
        let (cmd, args) = parse_command("/mode explore");
        assert_eq!(cmd, "/mode");
        assert_eq!(args, "explore");
    }

    #[test]
    fn parse_command_with_multi_args() {
        let (cmd, args) = parse_command("/commit fix the bug");
        assert_eq!(cmd, "/commit");
        assert_eq!(args, "fix the bug");
    }

    #[test]
    fn parse_command_trims_args() {
        let (cmd, args) = parse_command("/mode   explore  ");
        assert_eq!(cmd, "/mode");
        assert_eq!(args, "explore");
    }

    #[test]
    fn format_help_includes_commands() {
        let text = format_help_text();
        assert!(text.contains("/help"));
        assert!(text.contains("/quit"));
        assert!(text.contains("/mode"));
        assert!(text.contains("!<command>"));
    }
}

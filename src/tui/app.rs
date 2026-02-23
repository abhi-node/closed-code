use std::sync::Arc;

use ratatui::DefaultTerminal;

use crate::agent::orchestrator::Orchestrator;
use crate::config::Config;
use crate::gemini::GeminiClient;
use crate::mode::Mode;
use crate::sandbox::create_sandbox;
use crate::session::store::SessionStore;
use crate::session::{SessionEvent, SessionId};
use crate::ui::approval::DiffOnlyApprovalHandler;

use super::command_picker::CommandPicker;
use super::events::{self, AppEvent};
use super::input::InputPane;
use super::keybindings::{self, Action};
use super::layout;

/// Application state machine.
///
/// Phase 9b: Idle, CommandPicker, Exiting.
/// Phase 9c adds: Thinking, Streaming, ToolExecuting.
/// Phase 9d adds: AwaitingApproval, DiffView, SessionPicker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Idle,
    CommandPicker { filter: String, selected: usize },
    Exiting,
}

/// Snapshot of orchestrator state needed for rendering.
pub struct StatusSnapshot {
    pub mode: Mode,
    pub model: String,
    pub turn_count: usize,
    pub context_window_turns: usize,
    pub session_id: Option<SessionId>,
    pub git_branch: Option<String>,
    pub git_change_count: usize,
    pub git_is_clean: bool,
}

impl StatusSnapshot {
    pub fn from_orchestrator(orch: &Orchestrator) -> Self {
        let summary = orch.git_summary();
        let (git_branch, git_change_count, git_is_clean) = parse_git_summary(&summary);

        Self {
            mode: *orch.mode(),
            model: orch.model().to_string(),
            turn_count: orch.turn_count(),
            context_window_turns: orch.context_window_turns(),
            session_id: orch.session_id().cloned(),
            git_branch,
            git_change_count,
            git_is_clean,
        }
    }
}

/// Parse `GitContext::summary()` into structured fields.
fn parse_git_summary(summary: &str) -> (Option<String>, usize, bool) {
    if summary == "not a git repository" {
        return (None, 0, true);
    }
    let branch = summary.split(' ').next().map(String::from);
    let is_clean = summary.contains("(clean)");
    let change_count = if is_clean {
        0
    } else {
        summary
            .split('(')
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0)
    };
    (branch, change_count, is_clean)
}

pub struct App<'a> {
    pub state: AppState,
    pub tick_count: usize,
    pub status: StatusSnapshot,
    pub input_pane: InputPane<'a>,
    pub command_picker: CommandPicker,
    pub pending_input: Option<String>,
}

impl<'a> App<'a> {
    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Exit => {
                self.state = AppState::Exiting;
            }
            Action::Cancel => {
                if matches!(self.state, AppState::CommandPicker { .. }) {
                    self.state = AppState::Idle;
                    self.input_pane.clear();
                } else {
                    self.input_pane.clear();
                }
            }
            Action::Redraw => {} // Next frame will redraw

            // ── Input actions ──
            Action::Submit => {
                if let Some(text) = self.input_pane.submit() {
                    self.pending_input = Some(text);
                    // Phase 9c: dispatch to orchestrator
                }
            }
            Action::InsertNewline => {
                self.input_pane.insert_newline();
            }
            Action::InsertChar(c) => {
                if c == '/' && self.input_pane.is_empty() {
                    // Trigger command picker
                    self.input_pane.insert_char('/');
                    self.state = AppState::CommandPicker {
                        filter: String::new(),
                        selected: 0,
                    };
                } else {
                    self.input_pane.insert_char(c);
                }
            }
            Action::Backspace => {
                self.input_pane.delete_char_before();
            }
            Action::Delete => {
                self.input_pane.delete_char_at();
            }
            Action::ClearInput => {
                self.input_pane.clear();
                if matches!(self.state, AppState::CommandPicker { .. }) {
                    self.state = AppState::Idle;
                }
            }
            Action::OpenEditor => {
                if let Err(e) = self.input_pane.open_editor() {
                    tracing::warn!("Editor error: {}", e);
                }
            }
            Action::CursorLeft => self.input_pane.move_cursor_left(),
            Action::CursorRight => self.input_pane.move_cursor_right(),
            Action::CursorHome => self.input_pane.move_cursor_home(),
            Action::CursorEnd => self.input_pane.move_cursor_end(),

            // ── History ──
            Action::HistoryPrev => {
                if self.input_pane.is_empty() || self.input_pane.is_cycling_history() {
                    self.input_pane.history_prev();
                }
                // If input has content and not cycling, could scroll chat (Phase 9c)
            }
            Action::HistoryNext => {
                if self.input_pane.is_cycling_history() {
                    self.input_pane.history_next();
                }
            }

            // ── Chat scrolling (Phase 9c) ──
            Action::PageUp
            | Action::PageDown
            | Action::ScrollUp
            | Action::ScrollDown
            | Action::ScrollToTop
            | Action::ScrollToBottom => {
                // No-op in Phase 9b; wired in Phase 9c
            }

            // ── Command picker ──
            Action::PickerUp => {
                if let AppState::CommandPicker {
                    ref mut selected, ..
                } = self.state
                {
                    *selected = selected.saturating_sub(1);
                }
            }
            Action::PickerDown => {
                if let AppState::CommandPicker {
                    ref filter,
                    ref mut selected,
                    ..
                } = self.state
                {
                    let count = self.command_picker.filtered_count(filter);
                    if *selected + 1 < count {
                        *selected += 1;
                    }
                }
            }
            Action::PickerSelect => {
                if let AppState::CommandPicker {
                    ref filter,
                    selected,
                    ..
                } = self.state
                {
                    if let Some(cmd) = self.command_picker.get_selected(filter, selected) {
                        let text = if cmd.args.is_empty() {
                            cmd.name.to_string()
                        } else {
                            format!("{} ", cmd.name)
                        };
                        self.input_pane.clear();
                        self.input_pane.set_text(&text);
                    }
                }
                self.state = AppState::Idle;
            }
            Action::PickerDismiss => {
                self.state = AppState::Idle;
                self.input_pane.clear();
            }
            Action::PickerBackspace => {
                let text = self.input_pane.text();
                if text.len() <= 1 {
                    // Backspace past `/` — close picker
                    self.state = AppState::Idle;
                    self.input_pane.clear();
                } else {
                    self.input_pane.delete_char_before();
                    if let AppState::CommandPicker {
                        ref mut filter,
                        ref mut selected,
                        ..
                    } = self.state
                    {
                        let new_text = self.input_pane.text();
                        *filter = new_text
                            .strip_prefix('/')
                            .unwrap_or("")
                            .to_string();
                        *selected = 0;
                    }
                }
            }
            Action::PickerFilter(c) => {
                self.input_pane.insert_char(c);
                if let AppState::CommandPicker {
                    ref mut filter,
                    ref mut selected,
                    ..
                } = self.state
                {
                    filter.push(c);
                    *selected = 0;
                }
            }

            Action::Noop => {}
        }
    }
}

fn setup_terminal() -> anyhow::Result<DefaultTerminal> {
    // Install panic hook that restores terminal before printing panic info.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        ratatui::restore();
        original_hook(panic_info);
    }));

    let terminal = ratatui::init();
    Ok(terminal)
}

fn restore_terminal() {
    ratatui::restore();
}

pub async fn run(config: &Config) -> anyhow::Result<()> {
    // ── Build Orchestrator (mirrors run_repl setup) ──
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let sandbox = create_sandbox(config.sandbox_mode, config.working_directory.clone());
    let approval_handler: Arc<dyn crate::ui::approval::ApprovalHandler> =
        Arc::new(DiffOnlyApprovalHandler::new());

    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
        approval_handler,
        config.personality,
        config.context_window_turns,
        sandbox,
        config.protected_paths.clone(),
    );
    orchestrator.detect_git_context().await;

    if config.session_auto_save {
        let store = SessionStore::new(config.sessions_dir.clone());
        orchestrator.start_session(store);
    }

    // ── Terminal setup ──
    let mut terminal = setup_terminal()?;

    // ── App state ──
    let mut app = App {
        state: AppState::Idle,
        tick_count: 0,
        status: StatusSnapshot::from_orchestrator(&orchestrator),
        input_pane: InputPane::new(config.working_directory.clone()),
        command_picker: CommandPicker::new(),
        pending_input: None,
    };

    // ── Event loop ──
    let mut event_rx = events::spawn_event_loop();

    loop {
        terminal.draw(|frame| layout::render(frame, &mut app))?;

        let Some(event) = event_rx.recv().await else {
            break;
        };

        match event {
            AppEvent::Key(key) => {
                let action = keybindings::map_key(key, &app.state);
                app.handle_action(action);
            }
            AppEvent::Resize(_w, _h) => {
                // Ratatui handles resize on next draw automatically.
            }
            AppEvent::Tick => {
                app.tick_count = app.tick_count.wrapping_add(1);
            }
        }

        if app.state == AppState::Exiting {
            break;
        }
    }

    // ── Cleanup ──
    orchestrator.emit_event(SessionEvent::SessionEnd {
        timestamp: chrono::Utc::now(),
    });
    restore_terminal();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_git_summary_clean() {
        let (branch, count, clean) = parse_git_summary("main (clean)");
        assert_eq!(branch, Some("main".to_string()));
        assert_eq!(count, 0);
        assert!(clean);
    }

    #[test]
    fn parse_git_summary_changes() {
        let (branch, count, clean) = parse_git_summary("feature (3 uncommitted changes)");
        assert_eq!(branch, Some("feature".to_string()));
        assert_eq!(count, 3);
        assert!(!clean);
    }

    #[test]
    fn parse_git_summary_single_change() {
        let (branch, count, clean) = parse_git_summary("main (1 uncommitted change)");
        assert_eq!(branch, Some("main".to_string()));
        assert_eq!(count, 1);
        assert!(!clean);
    }

    #[test]
    fn parse_git_summary_not_repo() {
        let (branch, count, clean) = parse_git_summary("not a git repository");
        assert_eq!(branch, None);
        assert_eq!(count, 0);
        assert!(clean);
    }

    #[test]
    fn app_state_idle_is_not_exiting() {
        assert_ne!(AppState::Idle, AppState::Exiting);
    }
}

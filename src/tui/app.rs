use std::sync::Arc;

use ratatui::DefaultTerminal;
use tokio::sync::oneshot;

use crate::agent::orchestrator::{Orchestrator, OrchestratorConfig};
use crate::config::Config;
use crate::gemini::GeminiClient;
use crate::mode::Mode;
use crate::sandbox::create_sandbox;
use crate::session::store::SessionStore;
use crate::session::{SessionEvent, SessionId};
use crate::ui::approval::ApprovalDecision;

use super::approval_overlay::ApprovalOverlay;
use super::chat::{ChatMessage, ChatViewport};
use super::command_picker::CommandPicker;
use super::commands::{self, CommandResult};
use super::diff_view::DiffView;
use super::events::{self, AppEvent};
use super::file_completion::{self, FileCompletion};
use super::input::InputPane;
use super::keybindings::{self, Action};
use super::layout;
use super::message::SystemSeverity;
use super::mode_picker::ModePicker;
use super::session_picker::SessionPicker;
use super::tui_approval_handler::TuiApprovalHandler;

/// Application state machine.
///
/// Phase 9b: Idle, CommandPicker, Exiting.
/// Phase 9c adds: Thinking, Streaming, ToolExecuting.
/// Phase 9d adds: AwaitingApproval, DiffView, SessionPicker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Idle,
    CommandPicker { filter: String, selected: usize },
    Thinking,
    Streaming,
    ToolExecuting { count: usize },
    AwaitingApproval,
    DiffView,
    SessionPicker,
    ModePicker { confirming_auto: bool },
    CommitConfirm,
    Exiting,
}

/// Snapshot of orchestrator state needed for rendering.
pub struct StatusSnapshot {
    pub mode: Mode,
    pub model: String,
    pub turn_count: usize,
    pub context_window_turns: usize,
    pub last_prompt_tokens: u32,
    pub context_limit_tokens: u32,
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
            last_prompt_tokens: orch.last_prompt_tokens(),
            context_limit_tokens: orch.context_limit_tokens(),
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
    pub messages: Vec<ChatMessage>,
    pub chat_viewport: ChatViewport,
    // Phase 9d overlays
    pub approval_overlay: Option<ApprovalOverlay>,
    pub approval_response_tx: Option<oneshot::Sender<ApprovalDecision>>,
    pub diff_view_state: Option<DiffView>,
    pub session_picker: Option<SessionPicker>,
    pub mode_picker: Option<ModePicker>,
    // Commit confirmation
    pub commit_message: Option<String>,
    pub commit_working_dir: Option<std::path::PathBuf>,
    // Phase 9e: Performance & health
    pub message_line_cache: Vec<usize>,
    pub rate_limit_until: Option<std::time::Instant>,
    pub git_refresh_pending: bool,
    pub file_completion: Option<FileCompletion>,
}

impl<'a> App<'a> {
    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Exit => {
                self.state = AppState::Exiting;
            }
            Action::Cancel => {
                match &self.state {
                    AppState::CommandPicker { .. } => {
                        self.state = AppState::Idle;
                        self.input_pane.clear();
                    }
                    AppState::Thinking | AppState::Streaming | AppState::ToolExecuting { .. } => {
                        // Cancellation is handled by setting the cancel flag on the orchestrator.
                        // The spawned streaming task checks this flag and will send OrchestratorDone.
                        // We add an "Interrupted." message and transition to Idle.
                        self.messages.push(ChatMessage::system(SystemSeverity::Info, "Interrupted."));
                        self.state = AppState::Idle;
                    }
                    _ => {
                        self.input_pane.clear();
                    }
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

            // ── Chat scrolling ──
            Action::PageUp => self.chat_viewport.page_up(),
            Action::PageDown => self.chat_viewport.page_down(),
            Action::ScrollUp => self.chat_viewport.scroll_up(3),
            Action::ScrollDown => self.chat_viewport.scroll_down(3),
            Action::ScrollToTop => self.chat_viewport.scroll_to_top(),
            Action::ScrollToBottom => self.chat_viewport.scroll_to_bottom(),

            // ── File completion ──
            Action::TabComplete => {
                if let Some(ref mut fc) = self.file_completion {
                    fc.next();
                    if let Some(candidate) = fc.selected_candidate().map(|s| s.to_string()) {
                        let start_col = fc.start_col;
                        self.input_pane.apply_completion(start_col, &candidate);
                    }
                } else if let Some((word, start_col)) = self.input_pane.word_before_cursor() {
                    if file_completion::is_path_like(&word) && !word.starts_with('/') {
                        let wd = self.input_pane.working_directory().to_path_buf();
                        if let Some(mut fc) = FileCompletion::from_prefix(&word, &wd) {
                            fc.start_col = start_col;
                            if let Some(candidate) = fc.selected_candidate().map(|s| s.to_string()) {
                                self.input_pane.apply_completion(start_col, &candidate);
                            }
                            self.file_completion = Some(fc);
                        }
                    }
                }
                return; // Don't clear completion
            }
            Action::TabCompletePrev => {
                if let Some(ref mut fc) = self.file_completion {
                    fc.prev();
                    if let Some(candidate) = fc.selected_candidate().map(|s| s.to_string()) {
                        let start_col = fc.start_col;
                        self.input_pane.apply_completion(start_col, &candidate);
                    }
                }
                return; // Don't clear completion
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
                        *filter = new_text.strip_prefix('/').unwrap_or("").to_string();
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

            // ── Phase 9d: Approval overlay ──
            Action::ApprovalApprove => {
                if let Some(tx) = self.approval_response_tx.take() {
                    let _ = tx.send(ApprovalDecision::Approved);
                }
                let (file, diff) = self
                    .approval_overlay
                    .take()
                    .map(|o| (o.file_path, Some(o.diff_lines)))
                    .unwrap_or_default();
                self.messages.push(ChatMessage::System {
                    severity: SystemSeverity::Success,
                    text: format!("Approved: {}", file),
                    diff_lines: diff,
                });
                self.state = AppState::Thinking;
            }
            Action::ApprovalReject => {
                if let Some(tx) = self.approval_response_tx.take() {
                    let _ = tx.send(ApprovalDecision::Rejected);
                }
                let (file, diff) = self
                    .approval_overlay
                    .take()
                    .map(|o| (o.file_path, Some(o.diff_lines)))
                    .unwrap_or_default();
                self.messages.push(ChatMessage::System {
                    severity: SystemSeverity::Info,
                    text: format!("Rejected: {}", file),
                    diff_lines: diff,
                });
                self.state = AppState::Thinking;
            }
            Action::ApprovalOpenDiff => {
                if let Some(ref overlay) = self.approval_overlay {
                    self.diff_view_state = Some(DiffView::new(
                        overlay.file_path.clone(),
                        overlay.diff_lines.clone(),
                        overlay.additions,
                        overlay.deletions,
                    ));
                    self.state = AppState::DiffView;
                }
            }

            // ── Phase 9d: Diff view ──
            Action::DiffScrollUp => {
                if self.state == AppState::DiffView {
                    if let Some(ref mut view) = self.diff_view_state {
                        view.scroll_up(1);
                    }
                } else if self.state == AppState::AwaitingApproval {
                    if let Some(ref mut overlay) = self.approval_overlay {
                        overlay.scroll_up(1);
                    }
                }
            }
            Action::DiffScrollDown => {
                if self.state == AppState::DiffView {
                    if let Some(ref mut view) = self.diff_view_state {
                        view.scroll_down(1);
                    }
                } else if self.state == AppState::AwaitingApproval {
                    if let Some(ref mut overlay) = self.approval_overlay {
                        overlay.scroll_down(1, 20); // approximate visible
                    }
                }
            }
            Action::DiffHalfPageUp => {
                if let Some(ref mut view) = self.diff_view_state {
                    view.page_up();
                }
            }
            Action::DiffHalfPageDown => {
                if let Some(ref mut view) = self.diff_view_state {
                    view.page_down();
                }
            }
            Action::DiffTop => {
                if let Some(ref mut view) = self.diff_view_state {
                    view.scroll_to_top();
                }
            }
            Action::DiffBottom => {
                if let Some(ref mut view) = self.diff_view_state {
                    view.scroll_to_bottom();
                }
            }
            Action::DiffClose => {
                self.diff_view_state = None;
                self.state = AppState::AwaitingApproval;
            }

            // ── Phase 9d: List picker (shared session/mode) ──
            Action::ListUp => match self.state {
                AppState::SessionPicker => {
                    if let Some(ref mut picker) = self.session_picker {
                        picker.move_up();
                    }
                }
                AppState::ModePicker { .. } => {
                    if let Some(ref mut picker) = self.mode_picker {
                        picker.move_up();
                    }
                }
                _ => {}
            },
            Action::ListDown => match self.state {
                AppState::SessionPicker => {
                    if let Some(ref mut picker) = self.session_picker {
                        picker.move_down();
                    }
                }
                AppState::ModePicker { .. } => {
                    if let Some(ref mut picker) = self.mode_picker {
                        picker.move_down();
                    }
                }
                _ => {}
            },
            Action::ListSelect => {
                // Handled by run() because it needs orchestrator access
            }
            Action::ListDismiss => {
                match self.state {
                    AppState::SessionPicker => {
                        self.session_picker = None;
                        self.messages.push(ChatMessage::system(SystemSeverity::Info, "Cancelled."));
                    }
                    AppState::ModePicker { .. } => {
                        self.mode_picker = None;
                        self.messages.push(ChatMessage::system(SystemSeverity::Info, "Cancelled."));
                    }
                    _ => {}
                }
                self.state = AppState::Idle;
            }

            // ── Phase 9d: Mode confirmation ──
            Action::ModeConfirmYes => {
                // Handled by run() because it needs orchestrator access
            }
            Action::ModeConfirmNo => {
                if let Some(ref mut picker) = self.mode_picker {
                    picker.cancel_auto();
                }
                self.state = AppState::ModePicker {
                    confirming_auto: false,
                };
            }

            Action::Noop => {}
        }

        // Any non-Tab action clears file completion
        self.file_completion = None;
    }
}

/// Find the last Assistant message in the list (searching backwards).
///
/// This is needed because System messages (e.g., approval confirmations) may be
/// inserted between ToolStart and ToolComplete events, causing `last_mut()` to
/// return the wrong message.
fn last_assistant_mut(messages: &mut [ChatMessage]) -> Option<&mut ChatMessage> {
    messages
        .iter_mut()
        .rev()
        .find(|m| matches!(m, ChatMessage::Assistant { .. }))
}

fn setup_terminal() -> anyhow::Result<DefaultTerminal> {
    // Install panic hook that restores terminal before printing panic info.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        original_hook(panic_info);
    }));

    let terminal = ratatui::init();
    // Enable mouse capture for scroll support
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    Ok(terminal)
}

fn restore_terminal() {
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
}

pub async fn run(config: &Config, session_id: Option<&str>) -> anyhow::Result<()> {
    use crate::agent::orchestrator::OrchestratorEvent;
    use crate::gemini::stream::StreamEvent;
    use std::sync::atomic::Ordering;
    use tokio::sync::Mutex;

    // ── Event channels (created early so TuiApprovalHandler can use app_event_tx) ──
    let (app_event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

    // ── Build Orchestrator ──
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let sandbox = create_sandbox(config.sandbox_mode, config.working_directory.clone());
    let approval_handler: Arc<dyn crate::ui::approval::ApprovalHandler> =
        Arc::new(TuiApprovalHandler::new(app_event_tx.clone()));

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

    // Session setup: resume existing or start new
    let mut initial_messages: Vec<ChatMessage> = Vec::new();
    if let Some(id_str) = session_id {
        let store = SessionStore::new(config.sessions_dir.clone());
        let sid = match SessionId::parse(id_str) {
            Ok(id) => id,
            Err(_) => store.find_by_prefix(id_str)?,
        };
        let events = store.load_events(&sid)?;
        let history = SessionStore::reconstruct_history(&events);
        let turn_count = history.len();
        orchestrator.set_history(history);
        orchestrator.set_session(sid, store);
        initial_messages.push(ChatMessage::system(SystemSeverity::Success, format!("Resumed session ({} turns restored).", turn_count)));
    } else if config.session_auto_save {
        let store = SessionStore::new(config.sessions_dir.clone());
        orchestrator.start_session(store);
    }

    // Configure orchestrator for TUI mode
    orchestrator.set_suppress_display(true);

    // Orchestrator event channel (for tool/agent notifications)
    let (orch_event_tx, mut orch_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<OrchestratorEvent>();
    orchestrator.set_event_sender(orch_event_tx);

    // Extract cancel flag before wrapping in Arc<Mutex>
    let cancel_flag = orchestrator.cancel_flag();

    // Wrap orchestrator for shared access
    let orchestrator = Arc::new(Mutex::new(orchestrator));

    // ── Terminal setup ──
    let mut terminal = setup_terminal()?;

    // ── App state ──
    let initial_status = {
        let orch = orchestrator.lock().await;
        StatusSnapshot::from_orchestrator(&orch)
    };

    let mut app = App {
        state: AppState::Idle,
        tick_count: 0,
        status: initial_status,
        input_pane: InputPane::new(config.working_directory.clone()),
        command_picker: CommandPicker::new(),
        pending_input: None,
        messages: Vec::new(),
        chat_viewport: ChatViewport::new(),
        approval_overlay: None,
        approval_response_tx: None,
        diff_view_state: None,
        session_picker: None,
        mode_picker: None,
        commit_message: None,
        commit_working_dir: None,
        message_line_cache: Vec::new(),
        rate_limit_until: None,
        git_refresh_pending: false,
        file_completion: None,
    };

    // Inject initial messages (e.g. session resume notification)
    app.messages.extend(initial_messages);

    // ── Event loop (terminal reader + tick timer) ──
    events::spawn_event_loop(app_event_tx.clone());

    // Bridge: OrchestratorEvent -> AppEvent
    let bridge_tx = app_event_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = orch_event_rx.recv().await {
            let app_event = match event {
                OrchestratorEvent::ToolStart { tool_call_id, name, args_display } => {
                    AppEvent::ToolStart { tool_call_id, name, args_display }
                }
                OrchestratorEvent::ToolComplete { tool_call_id, name, duration } => {
                    AppEvent::ToolComplete { tool_call_id, name, duration }
                }
                OrchestratorEvent::ToolError { tool_call_id, name, error } => AppEvent::ToolError { tool_call_id, name, error },
                OrchestratorEvent::AgentStart { agent_type, task } => {
                    AppEvent::AgentStart { agent_type, task }
                }
                OrchestratorEvent::AgentComplete {
                    agent_type,
                    duration,
                } => AppEvent::AgentComplete {
                    agent_type,
                    duration,
                },
                OrchestratorEvent::AgentToolUpdate {
                    agent_type,
                    tool_name,
                    args_display,
                } => AppEvent::AgentToolUpdate {
                    agent_type,
                    tool_name,
                    args_display,
                },
            };
            if bridge_tx.send(app_event).is_err() {
                break;
            }
        }
    });

    // ── Event loop ──
    loop {
        terminal.draw(|frame| layout::render(frame, &mut app))?;

        // Process pending input (slash commands, shell commands, or LLM messages)
        if let Some(input) = app.pending_input.take() {
            if input.starts_with('/') {
                // Slash command — lock orchestrator briefly
                let mut orch = orchestrator.lock().await;
                let (msgs, result) =
                    commands::dispatch(&input, &mut orch, Some(&app_event_tx)).await;
                app.messages.extend(msgs);
                app.status = StatusSnapshot::from_orchestrator(&orch);
                drop(orch);

                match result {
                    CommandResult::Quit => {
                        app.state = AppState::Exiting;
                    }
                    CommandResult::ExecutePlan => {
                        // Kick off streaming with the plan
                        let orch_clone = orchestrator.clone();
                        let tx = app_event_tx.clone();
                        let flag = cancel_flag.clone();
                        flag.store(false, Ordering::SeqCst);
                        app.state = AppState::Thinking;
                        app.messages.push(ChatMessage::Assistant {
                            text: String::new(),
                            tool_calls: Vec::new(),
                            is_streaming: true,
                        });

                        tokio::spawn(async move {
                            let mut orch = orch_clone.lock().await;
                            let result = orch
                                .handle_user_input_streaming(
                                    "Execute the accepted plan step by step.",
                                    |event| match &event {
                                        StreamEvent::TextDelta(text) => {
                                            let _ = tx.send(AppEvent::TextDelta(text.clone()));
                                        }
                                        StreamEvent::Done { .. } => {
                                            let _ = tx.send(AppEvent::StreamDone);
                                        }
                                        _ => {}
                                    },
                                )
                                .await;
                            drop(orch);
                            match result {
                                Ok(_) => {
                                    let _ = tx.send(AppEvent::OrchestratorDone);
                                }
                                Err(e) => {
                                    let _ = tx.send(AppEvent::Error(e.to_string()));
                                    let _ = tx.send(AppEvent::OrchestratorDone);
                                }
                            }
                        });
                    }
                    CommandResult::SwitchMode(mode) => {
                        app.messages.push(ChatMessage::system(SystemSeverity::Info, format!("Switched to {} mode.", mode)));
                    }
                    CommandResult::ShowSessionPicker => {
                        app.state = AppState::SessionPicker;
                        // Load sessions asynchronously
                        let orch_clone = orchestrator.clone();
                        let tx = app_event_tx.clone();
                        tokio::spawn(async move {
                            let orch = orch_clone.lock().await;
                            if let Some(store) = orch.session_store() {
                                match store.list_sessions() {
                                    Ok(sessions) => {
                                        let _ = tx.send(AppEvent::SessionsLoaded(sessions));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(AppEvent::Error(format!(
                                            "Failed to list sessions: {}",
                                            e
                                        )));
                                        let _ = tx.send(AppEvent::SystemMessage(
                                            "No sessions found.".into(),
                                        ));
                                    }
                                }
                            } else {
                                let _ = tx.send(AppEvent::SystemMessage(
                                    "No session store configured.".into(),
                                ));
                            }
                        });
                    }
                    CommandResult::ShowModePicker => {
                        app.mode_picker = Some(ModePicker::new());
                        app.state = AppState::ModePicker {
                            confirming_auto: false,
                        };
                    }
                    CommandResult::RunCommitAgent { diff, working_dir } => {
                        app.messages.push(ChatMessage::Assistant {
                            text: String::new(),
                            tool_calls: Vec::new(),
                            is_streaming: true,
                        });
                        app.state = AppState::Thinking;

                        let orch_clone = orchestrator.clone();
                        let tx = app_event_tx.clone();
                        tokio::spawn(async move {
                            let start = std::time::Instant::now();
                            let _ = tx.send(AppEvent::AgentStart {
                                agent_type: "commit".into(),
                                task: "Generating commit message...".into(),
                            });

                            let orch = orch_clone.lock().await;
                            let result = orch.run_commit_agent(&diff).await;
                            drop(orch);

                            let _ = tx.send(AppEvent::AgentComplete {
                                agent_type: "commit".into(),
                                duration: start.elapsed(),
                            });
                            let _ = tx.send(AppEvent::StreamDone);

                            match result {
                                Ok(msg) => {
                                    let msg = msg.trim().trim_matches('"').to_string();
                                    let _ = tx.send(AppEvent::CommitReady {
                                        message: msg,
                                        working_dir,
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send(AppEvent::Error(format!(
                                        "Error generating commit message: {}",
                                        e
                                    )));
                                    let _ = tx.send(AppEvent::OrchestratorDone);
                                }
                            }
                        });
                    }
                    CommandResult::RunReviewAgent { diff, working_dir } => {
                        let _ = working_dir; // not needed for review
                        app.messages.push(ChatMessage::Assistant {
                            text: String::new(),
                            tool_calls: Vec::new(),
                            is_streaming: true,
                        });
                        app.state = AppState::Thinking;

                        let orch_clone = orchestrator.clone();
                        let tx = app_event_tx.clone();
                        tokio::spawn(async move {
                            let start = std::time::Instant::now();
                            let _ = tx.send(AppEvent::AgentStart {
                                agent_type: "review".into(),
                                task: "Reviewing code changes...".into(),
                            });

                            let mut orch = orch_clone.lock().await;
                            let result = orch.run_review_agent(&diff).await;
                            drop(orch);

                            let _ = tx.send(AppEvent::AgentComplete {
                                agent_type: "review".into(),
                                duration: start.elapsed(),
                            });

                            match result {
                                Ok(review) => {
                                    let _ = tx.send(AppEvent::TextDelta(review));
                                    let _ = tx.send(AppEvent::StreamDone);
                                    let _ = tx.send(AppEvent::SystemMessage(
                                        "(Review added to context)".into(),
                                    ));
                                }
                                Err(e) => {
                                    let _ = tx.send(AppEvent::StreamDone);
                                    let _ =
                                        tx.send(AppEvent::Error(format!("Review failed: {}", e)));
                                }
                            }
                            let _ = tx.send(AppEvent::OrchestratorDone);
                        });
                    }
                    CommandResult::RunCompact { user_prompt, turns_before } => {
                        app.state = AppState::Thinking;
                        app.messages.push(ChatMessage::system(
                            SystemSeverity::Info,
                            "Compacting history...",
                        ));

                        let orch_clone = orchestrator.clone();
                        let tx = app_event_tx.clone();
                        tokio::spawn(async move {
                            let mut orch = orch_clone.lock().await;
                            let result = orch.compact_history(user_prompt.as_deref()).await;
                            let turns_after = orch.turn_count();
                            drop(orch);

                            match result {
                                Ok(summary) => {
                                    let preview = if summary.len() > 200 {
                                        format!("{}...", &summary[..197])
                                    } else {
                                        summary
                                    };
                                    let _ = tx.send(AppEvent::SystemMessage(format!(
                                        "Compacted: {} turns -> {} turns\nSummary: {}",
                                        turns_before, turns_after, preview,
                                    )));
                                }
                                Err(e) => {
                                    let _ = tx.send(AppEvent::Error(format!("Compact failed: {}", e)));
                                }
                            }
                            let _ = tx.send(AppEvent::OrchestratorDone);
                        });
                    }
                    CommandResult::Continue => {}
                }
            } else if let Some(shell_input) = input.strip_prefix('!') {
                // Shell command
                let command = shell_input.trim().to_string();
                if !command.is_empty() {
                    let msg = commands::execute_shell_command(&command).await;
                    app.messages.push(msg);
                }
            } else {
                // Normal user message — send to LLM
                app.messages.push(ChatMessage::User {
                    text: input.clone(),
                });
                app.messages.push(ChatMessage::Assistant {
                    text: String::new(),
                    tool_calls: Vec::new(),
                    is_streaming: true,
                });
                app.state = AppState::Thinking;

                let orch_clone = orchestrator.clone();
                let tx = app_event_tx.clone();
                let flag = cancel_flag.clone();
                flag.store(false, Ordering::SeqCst);

                let input_owned = input;
                tokio::spawn(async move {
                    let mut orch = orch_clone.lock().await;
                    orch.reset_cancel();
                    let result = orch
                        .handle_user_input_streaming(&input_owned, |event| match &event {
                            StreamEvent::TextDelta(text) => {
                                let _ = tx.send(AppEvent::TextDelta(text.clone()));
                            }
                            StreamEvent::Done { .. } => {
                                let _ = tx.send(AppEvent::StreamDone);
                            }
                            _ => {}
                        })
                        .await;
                    drop(orch);
                    match result {
                        Ok(_) => {
                            let _ = tx.send(AppEvent::OrchestratorDone);
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Error(e.to_string()));
                            let _ = tx.send(AppEvent::OrchestratorDone);
                        }
                    }
                });
            }
        }

        let Some(event) = event_rx.recv().await else {
            break;
        };

        match event {
            AppEvent::Key(key) => {
                let action = keybindings::map_key(key, &app.state);

                // Handle cancel by setting the flag on the orchestrator
                if action == Action::Cancel
                    && matches!(
                        app.state,
                        AppState::Thinking | AppState::Streaming | AppState::ToolExecuting { .. }
                    )
                {
                    cancel_flag.store(true, Ordering::SeqCst);
                }

                // Actions that need orchestrator access
                match action {
                    Action::ListSelect if app.state == AppState::SessionPicker => {
                        if let Some(ref picker) = app.session_picker {
                            if let Some(meta) = picker.selected_session() {
                                let session_id = meta.session_id.clone();
                                app.session_picker = None;
                                app.state = AppState::Idle;
                                app.messages.push(ChatMessage::system(SystemSeverity::Info, format!(
                                    "Resuming session {}...",
                                    &session_id.as_str()[..8]
                                )));

                                let orch_clone = orchestrator.clone();
                                let tx = app_event_tx.clone();
                                tokio::spawn(async move {
                                    let mut orch = orch_clone.lock().await;
                                    // Load and restore session
                                    let result = (|| -> crate::error::Result<usize> {
                                        let store_ref = orch.session_store().ok_or_else(|| {
                                            crate::error::ClosedCodeError::SessionError(
                                                "No session store".into(),
                                            )
                                        })?;
                                        let events = store_ref.load_events(&session_id)?;
                                        let history = SessionStore::reconstruct_history(&events);
                                        let count = history.len();
                                        orch.set_history(history);
                                        Ok(count)
                                    })();
                                    match result {
                                        Ok(turns) => {
                                            let _ = tx.send(AppEvent::SystemMessage(format!(
                                                "Session {} resumed ({} turns restored).",
                                                &session_id.as_str()[..8],
                                                turns
                                            )));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(AppEvent::Error(format!(
                                                "Resume failed: {}",
                                                e
                                            )));
                                        }
                                    }
                                });
                            }
                        }
                        continue;
                    }
                    Action::ListSelect
                        if matches!(
                            app.state,
                            AppState::ModePicker {
                                confirming_auto: false
                            }
                        ) =>
                    {
                        if let Some(ref mut picker) = app.mode_picker {
                            if let Some(mode) = picker.try_select() {
                                // Accept plan with selected mode
                                let mut orch = orchestrator.lock().await;
                                if let Some(_plan) = orch.accept_plan(mode) {
                                    app.messages.push(ChatMessage::system(SystemSeverity::Success, format!("Plan accepted. Executing in {} mode.", mode)));
                                    app.status = StatusSnapshot::from_orchestrator(&orch);
                                }
                                drop(orch);
                                app.mode_picker = None;
                                app.state = AppState::Idle;
                                app.pending_input =
                                    Some("Execute the accepted plan step by step.".into());
                            } else {
                                // Auto selected — needs confirmation
                                app.state = AppState::ModePicker {
                                    confirming_auto: true,
                                };
                            }
                        }
                        continue;
                    }
                    Action::ModeConfirmYes if app.state == AppState::CommitConfirm => {
                        if let (Some(msg), Some(wd)) =
                            (app.commit_message.take(), app.commit_working_dir.take())
                        {
                            let orch_clone = orchestrator.clone();
                            let tx = app_event_tx.clone();
                            app.state = AppState::Thinking;
                            tokio::spawn(async move {
                                match crate::git::commit::commit_all(&wd, &msg).await {
                                    Ok(sha) => {
                                        let mut orch = orch_clone.lock().await;
                                        orch.refresh_git_context().await;
                                        drop(orch);
                                        let _ = tx.send(AppEvent::SystemMessage(format!(
                                            "Committed: {} ({})",
                                            sha, msg
                                        )));
                                    }
                                    Err(e) => {
                                        let _ = tx
                                            .send(AppEvent::Error(format!("Commit failed: {}", e)));
                                    }
                                }
                                let _ = tx.send(AppEvent::OrchestratorDone);
                            });
                        }
                        continue;
                    }
                    Action::ModeConfirmNo if app.state == AppState::CommitConfirm => {
                        app.commit_message = None;
                        app.commit_working_dir = None;
                        app.messages.push(ChatMessage::system(SystemSeverity::Info, "Commit cancelled."));
                        app.state = AppState::Idle;
                        continue;
                    }
                    Action::ModeConfirmYes => {
                        let mode = Mode::Auto;
                        let mut orch = orchestrator.lock().await;
                        if let Some(_plan) = orch.accept_plan(mode) {
                            app.messages.push(ChatMessage::system(SystemSeverity::Success, "Plan accepted. Executing in Auto mode."));
                            app.status = StatusSnapshot::from_orchestrator(&orch);
                        }
                        drop(orch);
                        app.mode_picker = None;
                        app.state = AppState::Idle;
                        app.pending_input = Some("Execute the accepted plan step by step.".into());
                        continue;
                    }
                    _ => {}
                }

                app.handle_action(action);
            }
            AppEvent::Resize(_w, _h) => {
                // Ratatui handles resize on next draw automatically.
                // Invalidate line cache — widths changed, line counts may differ.
                app.message_line_cache.clear();
            }
            AppEvent::MouseScrollUp => {
                app.chat_viewport.scroll_up(3);
            }
            AppEvent::MouseScrollDown => {
                app.chat_viewport.scroll_down(3);
            }
            AppEvent::Tick => {
                app.tick_count = app.tick_count.wrapping_add(1);
                // Rate-limit countdown
                if let Some(until) = app.rate_limit_until {
                    if std::time::Instant::now() >= until {
                        app.rate_limit_until = None;
                    }
                }
            }

            // ── Streaming events ──
            AppEvent::TextDelta(text) => {
                if app.state == AppState::Thinking {
                    app.state = AppState::Streaming;
                }
                // Append text to the last assistant message
                if let Some(ChatMessage::Assistant {
                    text: ref mut msg_text,
                    ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    msg_text.push_str(&text);
                }
            }
            AppEvent::StreamDone => {
                // Mark the last assistant message as not streaming
                if let Some(ChatMessage::Assistant {
                    ref mut is_streaming,
                    ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    *is_streaming = false;
                }
            }

            // ── Tool events ──
            AppEvent::ToolStart { tool_call_id, name, args_display } => {
                app.message_line_cache.clear();
                match app.state {
                    AppState::ToolExecuting { count } => {
                        app.state = AppState::ToolExecuting { count: count + 1 };
                    }
                    _ => {
                        app.state = AppState::ToolExecuting { count: 1 };
                    }
                }
                if let Some(ChatMessage::Assistant {
                    ref mut tool_calls, ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    tool_calls.push(super::chat::ToolCallDisplay::Running { tool_call_id, name, args_display });
                }
            }
            AppEvent::ToolComplete { tool_call_id, name, duration } => {
                app.message_line_cache.clear();
                if let Some(ChatMessage::Assistant {
                    ref mut tool_calls, ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    // Find the running tool call by ID and mark it completed
                    if let Some(tc) = tool_calls.iter_mut().rev().find(|tc| {
                        matches!(tc, super::chat::ToolCallDisplay::Running { tool_call_id: id, .. } if *id == tool_call_id)
                    }) {
                        *tc = super::chat::ToolCallDisplay::Completed { tool_call_id, name, duration };
                    }
                    // Transition to Thinking only if no other tools are still Running
                    let still_running = tool_calls.iter().any(|tc| {
                        matches!(tc, super::chat::ToolCallDisplay::Running { .. })
                    });
                    if !still_running {
                        app.state = AppState::Thinking;
                    } else if let AppState::ToolExecuting { count } = app.state {
                        app.state = AppState::ToolExecuting { count: count.saturating_sub(1).max(1) };
                    }
                }
            }
            AppEvent::ToolError { tool_call_id, name, error } => {
                app.message_line_cache.clear();
                if let Some(ChatMessage::Assistant {
                    ref mut tool_calls, ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    if let Some(tc) = tool_calls.iter_mut().rev().find(|tc| {
                        matches!(tc, super::chat::ToolCallDisplay::Running { tool_call_id: id, .. } if *id == tool_call_id)
                    }) {
                        *tc = super::chat::ToolCallDisplay::Failed { tool_call_id, name, error };
                    }
                    let still_running = tool_calls.iter().any(|tc| {
                        matches!(tc, super::chat::ToolCallDisplay::Running { .. })
                    });
                    if !still_running {
                        app.state = AppState::Thinking;
                    } else if let AppState::ToolExecuting { count } = app.state {
                        app.state = AppState::ToolExecuting { count: count.saturating_sub(1).max(1) };
                    }
                }
            }

            // ── Agent events ──
            AppEvent::AgentStart { agent_type, task } => {
                app.message_line_cache.clear();
                if let Some(ChatMessage::Assistant {
                    ref mut tool_calls, ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    tool_calls.push(super::chat::ToolCallDisplay::AgentRunning {
                        agent_type,
                        task,
                        last_tool: None,
                    });
                }
            }
            AppEvent::AgentComplete {
                agent_type,
                duration,
            } => {
                app.message_line_cache.clear();
                if let Some(ChatMessage::Assistant {
                    ref mut tool_calls, ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    if let Some(tc) = tool_calls.iter_mut().rev().find(|tc| {
                        matches!(tc, super::chat::ToolCallDisplay::AgentRunning { agent_type: at, .. } if *at == agent_type)
                    }) {
                        *tc = super::chat::ToolCallDisplay::AgentCompleted {
                            agent_type,
                            duration,
                        };
                    }
                }
            }

            AppEvent::AgentToolUpdate {
                agent_type,
                tool_name,
                args_display,
            } => {
                app.message_line_cache.clear();
                if let Some(ChatMessage::Assistant {
                    ref mut tool_calls, ..
                }) = last_assistant_mut(&mut app.messages)
                {
                    if let Some(super::chat::ToolCallDisplay::AgentRunning {
                        ref mut last_tool, ..
                    }) = tool_calls.iter_mut().rev().find(|tc| {
                        matches!(tc, super::chat::ToolCallDisplay::AgentRunning { agent_type: at, .. } if *at == agent_type)
                    }) {
                        *last_tool = Some(format!("{}({})", tool_name, args_display));
                    }
                }
            }

            // ── System events ──
            AppEvent::SystemMessage(text) => {
                app.messages.push(ChatMessage::system(SystemSeverity::Info, text));
            }
            AppEvent::ModeChanged(mode) => {
                app.messages.push(ChatMessage::system(SystemSeverity::Info, format!("Mode changed to: {}", mode)));
            }
            AppEvent::OrchestratorDone => {
                // Refresh status from orchestrator
                let mut orch = orchestrator.lock().await;
                // Capture plan text in Plan mode (matches repl.rs behavior)
                if *orch.mode() == crate::mode::Mode::Plan {
                    if let Some(ChatMessage::Assistant { ref text, .. }) = app.messages.iter().rev().find(|m| matches!(m, ChatMessage::Assistant { .. })) {
                        if !text.is_empty() {
                            orch.set_current_plan(text.clone());
                        }
                    }
                }
                app.status = StatusSnapshot::from_orchestrator(&orch);
                drop(orch);
                app.state = AppState::Idle;
            }
            AppEvent::Error(err) => {
                app.messages.push(ChatMessage::system(SystemSeverity::Error, format!("Error: {}", err)));
            }

            // ── Phase 9d: Overlay events ──
            AppEvent::ApprovalRequest {
                change,
                response_tx,
            } => {
                match app.status.mode {
                    Mode::Execute | Mode::Auto => {
                        // Auto-approve without showing overlay
                        let _ = response_tx.send(ApprovalDecision::Approved);
                        let overlay = ApprovalOverlay::from_change(&change);
                        app.messages.push(ChatMessage::System {
                            severity: SystemSeverity::Success,
                            text: format!("Auto-approved: {}", change.file_path),
                            diff_lines: Some(overlay.diff_lines),
                        });
                        // State remains unchanged (ToolExecuting/Thinking)
                    }
                    _ => {
                        // Guided/Explore/Plan: show approval overlay
                        app.approval_overlay = Some(ApprovalOverlay::from_change(&change));
                        app.approval_response_tx = Some(response_tx);
                        app.state = AppState::AwaitingApproval;
                    }
                }
            }
            AppEvent::CommitReady {
                message,
                working_dir,
            } => {
                app.messages.push(ChatMessage::system(SystemSeverity::Info, format!("Proposed commit message: \"{}\"", message)));
                app.commit_message = Some(message);
                app.commit_working_dir = Some(working_dir);
                app.state = AppState::CommitConfirm;
            }
            AppEvent::SessionsLoaded(sessions) => {
                if sessions.is_empty() {
                    app.messages.push(ChatMessage::system(SystemSeverity::Info, "No sessions found."));
                    app.state = AppState::Idle;
                } else {
                    app.session_picker = Some(SessionPicker::new(sessions));
                    // state is already SessionPicker from ShowSessionPicker handling
                }
            }

            // ── Phase 9e: Status & health events ──
            AppEvent::StatusUpdate(snapshot) => {
                app.status = snapshot;
            }
            AppEvent::RateLimited { retry_after_secs } => {
                app.rate_limit_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(retry_after_secs));
                app.messages.push(ChatMessage::system(SystemSeverity::Warning, format!("Rate limited. Retrying in {}s...", retry_after_secs)));
            }
            AppEvent::ContextPruned { turns_removed, turns_remaining } => {
                app.messages.push(ChatMessage::system(SystemSeverity::Warning, format!(
                    "Context pruned: {} turns removed, {} remaining.",
                    turns_removed, turns_remaining
                )));
            }
            AppEvent::ShellComplete(msg) => {
                app.messages.push(msg);
            }
        }

        if app.state == AppState::Exiting {
            break;
        }
    }

    // ── Cleanup ──
    {
        let orch = orchestrator.lock().await;
        orch.emit_event(SessionEvent::SessionEnd {
            timestamp: chrono::Utc::now(),
        });
    }
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

    #[test]
    fn app_state_thinking_variant() {
        assert_ne!(AppState::Thinking, AppState::Idle);
        assert_ne!(AppState::Streaming, AppState::Idle);
    }

    #[test]
    fn app_state_tool_executing() {
        let state = AppState::ToolExecuting { count: 1 };
        assert_ne!(state, AppState::Idle);
    }
}

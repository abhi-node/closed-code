use std::fmt;
use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent, MouseEventKind};
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use crate::ui::approval::{ApprovalDecision, FileChange};

pub const TICK_RATE: Duration = Duration::from_millis(100);

/// Application-level events consumed by the main event loop.
pub enum AppEvent {
    /// A key press from the terminal.
    Key(KeyEvent),
    /// Terminal resize (new columns, new rows).
    Resize(u16, u16),
    /// Periodic tick for animations (spinner frame advance).
    Tick,

    // ── Phase 9c: LLM Streaming ──
    /// A chunk of text from the streaming LLM response.
    TextDelta(String),
    /// Streaming is complete (final text available).
    StreamDone,

    // ── Phase 9c: Tool Events ──
    ToolStart {
        name: String,
        args_display: String,
    },
    ToolComplete {
        name: String,
        duration: Duration,
    },
    ToolError {
        name: String,
        error: String,
    },

    // ── Phase 9c: Sub-Agent Events ──
    AgentStart {
        agent_type: String,
        task: String,
    },
    AgentComplete {
        agent_type: String,
        duration: Duration,
    },
    AgentToolUpdate {
        agent_type: String,
        tool_name: String,
        args_display: String,
    },

    // ── Mouse Events ──
    MouseScrollUp,
    MouseScrollDown,

    // ── Phase 9c: System Events ──
    SystemMessage(String),
    ModeChanged(crate::mode::Mode),
    OrchestratorDone,
    Error(String),

    // ── Phase 9d: Overlay Events ──
    /// Approval request from the orchestrator (Guided mode file changes).
    ApprovalRequest {
        change: FileChange,
        response_tx: oneshot::Sender<ApprovalDecision>,
    },
    /// Sessions loaded asynchronously for the session picker.
    SessionsLoaded(Vec<crate::session::SessionMeta>),
    /// Commit agent finished — message ready for user confirmation.
    CommitReady {
        message: String,
        working_dir: std::path::PathBuf,
    },
}

// Manual Debug impl because oneshot::Sender doesn't implement Debug.
impl fmt::Debug for AppEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(key) => f.debug_tuple("Key").field(key).finish(),
            Self::Resize(w, h) => f.debug_tuple("Resize").field(w).field(h).finish(),
            Self::Tick => write!(f, "Tick"),
            Self::TextDelta(t) => f.debug_tuple("TextDelta").field(t).finish(),
            Self::StreamDone => write!(f, "StreamDone"),
            Self::ToolStart { name, .. } => {
                f.debug_struct("ToolStart").field("name", name).finish()
            }
            Self::ToolComplete { name, duration } => f
                .debug_struct("ToolComplete")
                .field("name", name)
                .field("duration", duration)
                .finish(),
            Self::ToolError { name, error } => f
                .debug_struct("ToolError")
                .field("name", name)
                .field("error", error)
                .finish(),
            Self::AgentStart { agent_type, task } => f
                .debug_struct("AgentStart")
                .field("agent_type", agent_type)
                .field("task", task)
                .finish(),
            Self::AgentComplete {
                agent_type,
                duration,
            } => f
                .debug_struct("AgentComplete")
                .field("agent_type", agent_type)
                .field("duration", duration)
                .finish(),
            Self::AgentToolUpdate {
                agent_type,
                tool_name,
                ..
            } => f
                .debug_struct("AgentToolUpdate")
                .field("agent_type", agent_type)
                .field("tool_name", tool_name)
                .finish(),
            Self::MouseScrollUp => write!(f, "MouseScrollUp"),
            Self::MouseScrollDown => write!(f, "MouseScrollDown"),
            Self::SystemMessage(msg) => f.debug_tuple("SystemMessage").field(msg).finish(),
            Self::ModeChanged(mode) => f.debug_tuple("ModeChanged").field(mode).finish(),
            Self::OrchestratorDone => write!(f, "OrchestratorDone"),
            Self::Error(err) => f.debug_tuple("Error").field(err).finish(),
            Self::ApprovalRequest { change, .. } => f
                .debug_struct("ApprovalRequest")
                .field("file_path", &change.file_path)
                .finish(),
            Self::SessionsLoaded(sessions) => f
                .debug_tuple("SessionsLoaded")
                .field(&sessions.len())
                .finish(),
            Self::CommitReady { message, .. } => f
                .debug_struct("CommitReady")
                .field("message", message)
                .finish(),
        }
    }
}

/// Spawn a background task that polls crossterm events and a tick timer,
/// sending `AppEvent` values into the provided sender.
///
/// Returns the receiver. The caller can also clone `tx` to inject
/// programmatic events (tool notifications, streaming deltas, etc.).
pub fn spawn_event_loop(tx: mpsc::UnboundedSender<AppEvent>) -> mpsc::UnboundedSender<AppEvent> {
    let tx_clone = tx.clone();

    tokio::spawn(async move {
        let mut reader = EventStream::new();
        let mut tick = tokio::time::interval(TICK_RATE);

        loop {
            let event = tokio::select! {
                maybe_event = reader.next() => {
                    match maybe_event {
                        Some(Ok(CrosstermEvent::Key(key))) => Some(AppEvent::Key(key)),
                        Some(Ok(CrosstermEvent::Resize(w, h))) => Some(AppEvent::Resize(w, h)),
                        Some(Ok(CrosstermEvent::Mouse(mouse))) => match mouse.kind {
                            MouseEventKind::ScrollUp => Some(AppEvent::MouseScrollUp),
                            MouseEventKind::ScrollDown => Some(AppEvent::MouseScrollDown),
                            _ => None,
                        },
                        Some(Ok(_)) => None,
                        Some(Err(_)) => None,  // Read error, skip
                        None => break,         // Stream ended
                    }
                }
                _ = tick.tick() => {
                    Some(AppEvent::Tick)
                }
            };

            if let Some(ev) = event {
                if tx_clone.send(ev).is_err() {
                    break; // Receiver dropped
                }
            }
        }
    });

    tx
}

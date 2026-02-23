use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent, MouseEventKind};
use futures::StreamExt;
use tokio::sync::mpsc;

pub const TICK_RATE: Duration = Duration::from_millis(100);

/// Application-level events consumed by the main event loop.
#[derive(Debug)]
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

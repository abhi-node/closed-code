use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent};
use futures::StreamExt;
use tokio::sync::mpsc;

pub const TICK_RATE: Duration = Duration::from_millis(100);

/// Application-level events consumed by the main event loop.
///
/// Phase 9a uses Key, Resize, and Tick.
/// Later phases add TextDelta, ToolStart, ApprovalRequest, etc.
#[derive(Debug)]
pub enum AppEvent {
    /// A key press from the terminal.
    Key(KeyEvent),
    /// Terminal resize (new columns, new rows).
    Resize(u16, u16),
    /// Periodic tick for animations (spinner frame advance).
    Tick,
}

/// Spawn a background task that polls crossterm events and a tick timer,
/// sending `AppEvent` values into the returned receiver.
///
/// The task exits when the receiver is dropped.
pub fn spawn_event_loop() -> mpsc::UnboundedReceiver<AppEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let mut reader = EventStream::new();
        let mut tick = tokio::time::interval(TICK_RATE);

        loop {
            let event = tokio::select! {
                maybe_event = reader.next() => {
                    match maybe_event {
                        Some(Ok(CrosstermEvent::Key(key))) => Some(AppEvent::Key(key)),
                        Some(Ok(CrosstermEvent::Resize(w, h))) => Some(AppEvent::Resize(w, h)),
                        Some(Ok(_)) => None,  // Mouse events ignored in 9a
                        Some(Err(_)) => None,  // Read error, skip
                        None => break,         // Stream ended
                    }
                }
                _ = tick.tick() => {
                    Some(AppEvent::Tick)
                }
            };

            if let Some(ev) = event {
                if tx.send(ev).is_err() {
                    break; // Receiver dropped
                }
            }
        }
    });

    rx
}

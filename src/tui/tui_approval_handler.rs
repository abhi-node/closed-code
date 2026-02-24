use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use crate::error::{ClosedCodeError, Result};
use crate::ui::approval::{ApprovalDecision, ApprovalHandler, FileChange};

use super::events::AppEvent;

/// TUI-based approval handler that bridges to the event loop via channels.
///
/// When the orchestrator requests file approval in Guided mode, this handler
/// sends an `AppEvent::ApprovalRequest` to the TUI event loop with a oneshot
/// channel for the response. The TUI displays the approval overlay and sends
/// the user's decision back through the channel.
#[derive(Debug)]
pub struct TuiApprovalHandler {
    event_tx: mpsc::UnboundedSender<AppEvent>,
}

impl TuiApprovalHandler {
    pub fn new(event_tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self { event_tx }
    }
}

#[async_trait]
impl ApprovalHandler for TuiApprovalHandler {
    async fn request_approval(&self, change: &FileChange) -> Result<ApprovalDecision> {
        let (response_tx, response_rx) = oneshot::channel();

        self.event_tx
            .send(AppEvent::ApprovalRequest {
                change: change.clone(),
                response_tx,
            })
            .map_err(|_| ClosedCodeError::ApprovalError("TUI event loop closed".into()))?;

        response_rx
            .await
            .map_err(|_| ClosedCodeError::ApprovalError("Approval response channel closed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_approval() {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let handler = TuiApprovalHandler::new(event_tx);

        let change = FileChange {
            file_path: "test.rs".into(),
            resolved_path: "/tmp/test.rs".into(),
            old_content: String::new(),
            new_content: "fn main() {}".into(),
            is_new_file: true,
        };

        // Spawn handler request
        let handle = tokio::spawn(async move { handler.request_approval(&change).await });

        // Receive event and respond
        if let Some(AppEvent::ApprovalRequest { response_tx, .. }) = event_rx.recv().await {
            response_tx.send(ApprovalDecision::Approved).unwrap();
        } else {
            panic!("Expected ApprovalRequest event");
        }

        let decision = handle.await.unwrap().unwrap();
        assert_eq!(decision, ApprovalDecision::Approved);
    }
}

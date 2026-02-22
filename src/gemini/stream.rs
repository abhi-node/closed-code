use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::{GenerateContentResponse, Part, UsageMetadata};

/// Events yielded to the REPL during streaming.
pub enum StreamEvent {
    /// A text chunk to display immediately.
    TextDelta(String),
    /// The complete response (final chunk with finish reason).
    Done {
        finish_reason: Option<String>,
        usage: Option<UsageMetadata>,
    },
    /// A function call was detected (Phase 2+). Contains the full response.
    FunctionCall(GenerateContentResponse),
}

/// Consume an EventSource and yield StreamEvents.
/// Collects the full assistant text for appending to conversation history.
pub async fn consume_stream(
    mut es: EventSource,
    on_event: impl Fn(StreamEvent),
) -> Result<String> {
    let mut full_text = String::new();

    while let Some(event) = es.next().await {
        match event {
            Ok(Event::Open) => {
                tracing::debug!("SSE connection opened");
            }
            Ok(Event::Message(msg)) => {
                let response: GenerateContentResponse = match serde_json::from_str(&msg.data) {
                    Ok(r) => r,
                    Err(e) => {
                        // Gemini occasionally sends malformed JSON (trailing commas)
                        // on the final chunk. If we already have text, treat as done.
                        if !full_text.is_empty() {
                            tracing::debug!("Ignoring malformed final SSE chunk: {e}");
                            on_event(StreamEvent::Done {
                                finish_reason: Some("STOP".into()),
                                usage: None,
                            });
                            break;
                        }
                        return Err(ClosedCodeError::StreamError(format!(
                            "Failed to parse SSE data: {e}"
                        )));
                    }
                };

                if let Some(candidate) = response.candidates.first() {
                    if let Some(content) = &candidate.content {
                        for part in &content.parts {
                            match part {
                                Part::Text(text) => {
                                    full_text.push_str(text);
                                    on_event(StreamEvent::TextDelta(text.clone()));
                                }
                                Part::FunctionCall { .. } => {
                                    on_event(StreamEvent::FunctionCall(response.clone()));
                                    es.close();
                                    return Ok(full_text);
                                }
                                _ => {}
                            }
                        }
                    }

                    if candidate.finish_reason.is_some() {
                        on_event(StreamEvent::Done {
                            finish_reason: candidate.finish_reason.clone(),
                            usage: response.usage_metadata.clone(),
                        });
                    }
                }
            }
            Err(reqwest_eventsource::Error::StreamEnded) => break,
            Err(e) => {
                es.close();
                return Err(ClosedCodeError::StreamError(e.to_string()));
            }
        }
    }

    Ok(full_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_event_variants_exist() {
        // Verify StreamEvent enum variants compile and can be matched
        let text_event = StreamEvent::TextDelta("hello".into());
        assert!(matches!(text_event, StreamEvent::TextDelta(_)));

        let done_event = StreamEvent::Done {
            finish_reason: Some("STOP".into()),
            usage: None,
        };
        assert!(matches!(done_event, StreamEvent::Done { .. }));
    }
}

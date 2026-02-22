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

/// Result of consuming a stream.
#[derive(Debug)]
pub enum StreamResult {
    /// Normal text completion.
    Text(String),
    /// A function call was detected. Contains the full response with function call parts,
    /// plus any text accumulated before the function call.
    FunctionCall {
        text_so_far: String,
        response: GenerateContentResponse,
    },
}

/// Strip outer JSON array artifacts from Gemini SSE data fields.
///
/// Gemini's `streamGenerateContent?alt=sse` may leak array wrapper artifacts
/// into individual SSE data fields: leading `[`, trailing `]`, and commas
/// between array elements.
fn strip_array_artifacts(raw: &str) -> &str {
    let s = raw.trim();
    let s = s.strip_prefix('[').unwrap_or(s).trim();
    let s = s.strip_suffix(']').unwrap_or(s).trim();
    let s = s.strip_suffix(',').unwrap_or(s).trim();
    s.strip_prefix(',').unwrap_or(s).trim()
}

/// Parse a JSON string leniently, handling Gemini's malformed SSE data.
///
/// Tries strict serde_json first (fast path). If that fails, falls back to
/// the json5 crate which natively handles trailing commas, a known Gemini issue.
fn parse_sse_json<T: serde::de::DeserializeOwned>(raw: &str) -> std::result::Result<T, String> {
    let data = strip_array_artifacts(raw);

    // Fast path: strict JSON
    if let Ok(val) = serde_json::from_str::<T>(data) {
        return Ok(val);
    }

    // Slow path: json5 handles trailing commas, comments, etc.
    json5::from_str::<T>(data).map_err(|e| {
        tracing::debug!("Both serde_json and json5 failed to parse SSE data");
        tracing::debug!("Raw SSE data: {:?}", raw);
        tracing::debug!("After stripping array artifacts: {:?}", data);
        format!("{e}")
    })
}

/// Consume an EventSource and yield StreamEvents.
/// Collects the full assistant text for appending to conversation history.
pub async fn consume_stream(
    mut es: EventSource,
    mut on_event: impl FnMut(StreamEvent),
) -> Result<StreamResult> {
    let mut full_text = String::new();

    while let Some(event) = es.next().await {
        match event {
            Ok(Event::Open) => {
                tracing::debug!("SSE connection opened");
            }
            Ok(Event::Message(msg)) => {
                tracing::trace!("Raw SSE data: {:?}", &msg.data);
                let response: GenerateContentResponse = match parse_sse_json(&msg.data) {
                    Ok(r) => r,
                    Err(e) => {
                        // If we already have text, treat as done gracefully.
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
                                    return Ok(StreamResult::FunctionCall {
                                        text_so_far: full_text,
                                        response,
                                    });
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

    Ok(StreamResult::Text(full_text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::types::{Candidate, Content};

    #[test]
    fn stream_event_variants_exist() {
        let text_event = StreamEvent::TextDelta("hello".into());
        assert!(matches!(text_event, StreamEvent::TextDelta(_)));

        let done_event = StreamEvent::Done {
            finish_reason: Some("STOP".into()),
            usage: None,
        };
        assert!(matches!(done_event, StreamEvent::Done { .. }));
    }

    #[test]
    fn stream_result_text_variant() {
        let result = StreamResult::Text("hello world".into());
        match result {
            StreamResult::Text(text) => assert_eq!(text, "hello world"),
            _ => panic!("Expected StreamResult::Text"),
        }
    }

    #[test]
    fn stream_result_function_call_variant() {
        let response = GenerateContentResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".into()),
                    parts: vec![Part::FunctionCall {
                        name: "read_file".into(),
                        args: serde_json::json!({"path": "src/main.rs"}),
                        thought_signature: None,
                    }],
                }),
                finish_reason: Some("STOP".into()),
                safety_ratings: vec![],
                grounding_metadata: None,
            }],
            usage_metadata: None,
            model_version: None,
        };
        let result = StreamResult::FunctionCall {
            text_so_far: "partial".into(),
            response,
        };
        match result {
            StreamResult::FunctionCall {
                text_so_far,
                response,
            } => {
                assert_eq!(text_so_far, "partial");
                assert!(response.has_function_calls());
            }
            _ => panic!("Expected StreamResult::FunctionCall"),
        }
    }

    #[test]
    fn strip_array_artifacts_clean() {
        assert_eq!(strip_array_artifacts(r#"{"key": "value"}"#), r#"{"key": "value"}"#);
    }

    #[test]
    fn strip_array_artifacts_trailing_comma() {
        assert_eq!(
            strip_array_artifacts(r#"{"key": "value"},"#),
            r#"{"key": "value"}"#
        );
    }

    #[test]
    fn strip_array_artifacts_brackets_and_comma() {
        assert_eq!(
            strip_array_artifacts(r#"[{"key": "value"},"#),
            r#"{"key": "value"}"#
        );
    }

    #[test]
    fn strip_array_artifacts_trailing_bracket() {
        assert_eq!(
            strip_array_artifacts(r#"{"key": "value"}]"#),
            r#"{"key": "value"}"#
        );
    }

    #[test]
    fn strip_array_artifacts_leading_comma() {
        assert_eq!(
            strip_array_artifacts(r#",{"key": "value"},"#),
            r#"{"key": "value"}"#
        );
    }

    #[test]
    fn strip_array_artifacts_whitespace() {
        assert_eq!(
            strip_array_artifacts("  [ {\"key\": \"value\"} , \n ] "),
            "{\"key\": \"value\"}"
        );
    }

    #[test]
    fn parse_sse_json_valid_json() {
        let input = r#"{"candidates":[]}"#;
        let result: GenerateContentResponse = parse_sse_json(input).unwrap();
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn parse_sse_json_trailing_comma_in_object() {
        // json5 fallback handles trailing commas
        let input = r#"{"candidates":[],"modelVersion":"v1",}"#;
        let result: GenerateContentResponse = parse_sse_json(input).unwrap();
        assert_eq!(result.model_version.as_deref(), Some("v1"));
    }

    #[test]
    fn parse_sse_json_trailing_comma_in_nested() {
        let input = r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}],"role":"model",},"finishReason":"STOP",}],"modelVersion":"v1",}"#;
        let result: GenerateContentResponse = parse_sse_json(input).unwrap();
        assert_eq!(result.text(), Some("Hi"));
    }

    #[test]
    fn parse_sse_json_array_wrapper_with_trailing_comma() {
        let input = r#"[{"candidates":[{"content":{"parts":[{"text":"Hi"}],"role":"model",},"finishReason":"STOP",}],"modelVersion":"v1",},"#;
        let result: GenerateContentResponse = parse_sse_json(input).unwrap();
        assert_eq!(result.text(), Some("Hi"));
    }
}

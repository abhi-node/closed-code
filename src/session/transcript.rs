use crate::error::Result;
use crate::session::SessionEvent;

/// Renders session events as a Markdown transcript.
pub struct TranscriptWriter;

impl TranscriptWriter {
    /// Render events to a Markdown string.
    pub fn render(events: &[SessionEvent]) -> String {
        let mut md = String::new();

        // Header
        md.push_str("# Session Transcript\n\n");

        // Extract metadata from SessionStart
        if let Some(SessionEvent::SessionStart {
            session_id,
            model,
            mode,
            working_directory,
            timestamp,
            ..
        }) = events.first()
        {
            md.push_str(&format!("- **Session**: {}\n", session_id));
            md.push_str(&format!("- **Model**: {}\n", model));
            md.push_str(&format!("- **Mode**: {}\n", mode));
            md.push_str(&format!("- **Directory**: {}\n", working_directory));
            md.push_str(&format!(
                "- **Started**: {}\n",
                timestamp.format("%Y-%m-%d %H:%M:%S UTC")
            ));
            md.push_str("\n---\n\n");
        }

        for event in events {
            match event {
                SessionEvent::SessionStart { .. } => {
                    // Already handled in header
                }
                SessionEvent::UserMessage {
                    content, timestamp, ..
                } => {
                    md.push_str(&format!(
                        "### User ({})\n\n{}\n\n",
                        timestamp.format("%H:%M:%S"),
                        content
                    ));
                }
                SessionEvent::AssistantMessage {
                    content, timestamp, ..
                } => {
                    md.push_str(&format!(
                        "### Assistant ({})\n\n{}\n\n",
                        timestamp.format("%H:%M:%S"),
                        content
                    ));
                }
                SessionEvent::ToolCall {
                    name,
                    args,
                    timestamp,
                } => {
                    let args_pretty =
                        serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
                    md.push_str(&format!(
                        "> **Tool Call** `{}` ({})\n>\n> ```json\n> {}\n> ```\n\n",
                        name,
                        timestamp.format("%H:%M:%S"),
                        args_pretty.replace('\n', "\n> ")
                    ));
                }
                SessionEvent::ToolResponse {
                    name,
                    result,
                    timestamp,
                } => {
                    let truncated = if result.len() > 500 {
                        format!("{}...", &result[..497])
                    } else {
                        result.clone()
                    };
                    md.push_str(&format!(
                        "> **Tool Response** `{}` ({})\n>\n> {}\n\n",
                        name,
                        timestamp.format("%H:%M:%S"),
                        truncated
                    ));
                }
                SessionEvent::ModeChange {
                    from,
                    to,
                    timestamp,
                    ..
                } => {
                    md.push_str(&format!(
                        "*Mode changed from {} to {} ({})*\n\n",
                        from,
                        to,
                        timestamp.format("%H:%M:%S")
                    ));
                }
                SessionEvent::Compact {
                    summary,
                    turns_before,
                    turns_after,
                    timestamp,
                } => {
                    md.push_str(&format!(
                        "---\n\n**Conversation Compacted** ({}) — {} turns → {} turns\n\n{}\n\n---\n\n",
                        timestamp.format("%H:%M:%S"),
                        turns_before,
                        turns_after,
                        summary
                    ));
                }
                SessionEvent::ImageAttached {
                    mime_type,
                    size_bytes,
                    timestamp,
                } => {
                    md.push_str(&format!(
                        "*Image attached: {} ({} bytes) ({})*\n\n",
                        mime_type,
                        size_bytes,
                        timestamp.format("%H:%M:%S")
                    ));
                }
                SessionEvent::SessionEnd { timestamp } => {
                    md.push_str(&format!(
                        "---\n\n*Session ended at {}*\n",
                        timestamp.format("%Y-%m-%d %H:%M:%S UTC")
                    ));
                }
            }
        }

        md
    }

    /// Write transcript to a file.
    pub fn write_to_file(events: &[SessionEvent], path: &str) -> Result<()> {
        let content = Self::render(events);
        std::fs::write(path, content).map_err(crate::error::ClosedCodeError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;
    use chrono::Utc;

    #[test]
    fn render_basic_conversation() {
        let ts = Utc::now();
        let events = vec![
            SessionEvent::SessionStart {
                session_id: SessionId::new(),
                model: "gemini-2.5-pro".into(),
                mode: "auto".into(),
                working_directory: "/tmp/project".into(),
                timestamp: ts,
            },
            SessionEvent::UserMessage {
                content: "Hello!".into(),
                timestamp: ts,
            },
            SessionEvent::AssistantMessage {
                content: "Hi there!".into(),
                timestamp: ts,
            },
            SessionEvent::SessionEnd { timestamp: ts },
        ];

        let md = TranscriptWriter::render(&events);
        assert!(md.contains("# Session Transcript"));
        assert!(md.contains("### User"));
        assert!(md.contains("Hello!"));
        assert!(md.contains("### Assistant"));
        assert!(md.contains("Hi there!"));
        assert!(md.contains("Session ended"));
    }

    #[test]
    fn render_with_tools() {
        let ts = Utc::now();
        let events = vec![
            SessionEvent::ToolCall {
                name: "read_file".into(),
                args: serde_json::json!({"path": "/tmp/test.rs"}),
                timestamp: ts,
            },
            SessionEvent::ToolResponse {
                name: "read_file".into(),
                result: "fn main() {}".into(),
                timestamp: ts,
            },
        ];

        let md = TranscriptWriter::render(&events);
        assert!(md.contains("**Tool Call** `read_file`"));
        assert!(md.contains("**Tool Response** `read_file`"));
        assert!(md.contains("fn main() {}"));
    }

    #[test]
    fn render_with_compact() {
        let ts = Utc::now();
        let events = vec![SessionEvent::Compact {
            summary: "User discussed database design.".into(),
            turns_before: 20,
            turns_after: 6,
            timestamp: ts,
        }];

        let md = TranscriptWriter::render(&events);
        assert!(md.contains("Conversation Compacted"));
        assert!(md.contains("20 turns"));
        assert!(md.contains("6 turns"));
        assert!(md.contains("database design"));
    }

    #[test]
    fn render_with_image() {
        let ts = Utc::now();
        let events = vec![SessionEvent::ImageAttached {
            mime_type: "image/png".into(),
            size_bytes: 54321,
            timestamp: ts,
        }];

        let md = TranscriptWriter::render(&events);
        assert!(md.contains("image/png"));
        assert!(md.contains("54321 bytes"));
    }

    #[test]
    fn render_empty_events() {
        let md = TranscriptWriter::render(&[]);
        assert!(md.contains("# Session Transcript"));
        // Should not crash, just header
    }

    #[test]
    fn render_tool_response_truncation() {
        let ts = Utc::now();
        let long_result = "x".repeat(1000);
        let events = vec![SessionEvent::ToolResponse {
            name: "read_file".into(),
            result: long_result,
            timestamp: ts,
        }];

        let md = TranscriptWriter::render(&events);
        assert!(md.contains("..."));
        // Should be truncated
        assert!(!md.contains(&"x".repeat(1000)));
    }

    #[test]
    fn render_mode_change() {
        let ts = Utc::now();
        let events = vec![SessionEvent::ModeChange {
            from: "explore".into(),
            to: "execute".into(),
            timestamp: ts,
        }];

        let md = TranscriptWriter::render(&events);
        assert!(md.contains("Mode changed from explore to execute"));
    }

    #[test]
    fn write_to_file_success() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("transcript.md");
        let ts = Utc::now();
        let events = vec![SessionEvent::UserMessage {
            content: "Test".into(),
            timestamp: ts,
        }];

        TranscriptWriter::write_to_file(&events, path.to_str().unwrap()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("Test"));
    }
}

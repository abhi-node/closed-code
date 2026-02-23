use std::path::PathBuf;

use crate::error::Result;
use crate::gemini::types::Content;
use crate::session::{SessionEvent, SessionId, SessionMeta};

/// JSONL-based session persistence store.
#[derive(Debug, Clone)]
pub struct SessionStore {
    sessions_dir: PathBuf,
}

impl SessionStore {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    /// Default sessions directory: `~/.closed-code/sessions/`.
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".closed-code")
            .join("sessions")
    }

    /// Ensure the sessions directory exists.
    pub fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.sessions_dir).map_err(crate::error::ClosedCodeError::Io)
    }

    /// Path to a session's JSONL file.
    pub fn session_path(&self, session_id: &SessionId) -> PathBuf {
        self.sessions_dir.join(format!("{}.jsonl", session_id))
    }

    /// Append a single event to a session file.
    pub fn save_event(&self, session_id: &SessionId, event: &SessionEvent) -> Result<()> {
        use std::io::Write;

        self.ensure_dir()?;
        let path = self.session_path(session_id);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(crate::error::ClosedCodeError::Io)?;

        let line = serde_json::to_string(event)?;
        writeln!(file, "{}", line).map_err(crate::error::ClosedCodeError::Io)?;
        file.flush().map_err(crate::error::ClosedCodeError::Io)?;
        Ok(())
    }

    /// Load all events from a session file.
    pub fn load_events(&self, session_id: &SessionId) -> Result<Vec<SessionEvent>> {
        use std::io::BufRead;

        let path = self.session_path(session_id);
        if !path.exists() {
            return Err(crate::error::ClosedCodeError::SessionNotFound(
                session_id.as_str(),
            ));
        }

        let file =
            std::fs::File::open(&path).map_err(crate::error::ClosedCodeError::Io)?;
        let reader = std::io::BufReader::new(file);
        let mut events = Vec::new();

        for line in reader.lines() {
            let line = line.map_err(crate::error::ClosedCodeError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEvent>(&line) {
                Ok(event) => events.push(event),
                Err(e) => {
                    tracing::warn!("Skipping malformed session event line: {}", e);
                }
            }
        }

        Ok(events)
    }

    /// List all sessions sorted by most recent activity.
    pub fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        self.ensure_dir()?;
        let mut sessions = Vec::new();

        let entries = std::fs::read_dir(&self.sessions_dir)
            .map_err(crate::error::ClosedCodeError::Io)?;

        for entry in entries {
            let entry = entry.map_err(crate::error::ClosedCodeError::Io)?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Ok(meta) = self.read_session_meta(&path) {
                    sessions.push(meta);
                }
            }
        }

        // Sort by most recent first
        sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        Ok(sessions)
    }

    /// Read metadata from a session file (first line for SessionStart, scan for preview + last timestamp).
    fn read_session_meta(&self, path: &std::path::Path) -> Result<SessionMeta> {
        use std::io::BufRead;

        let file = std::fs::File::open(path).map_err(crate::error::ClosedCodeError::Io)?;
        let reader = std::io::BufReader::new(file);

        let mut first_event: Option<SessionEvent> = None;
        let mut last_timestamp = None;
        let mut preview = String::new();

        for line in reader.lines() {
            let line = line.map_err(crate::error::ClosedCodeError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<SessionEvent>(&line) {
                last_timestamp = Some(event.timestamp());

                if first_event.is_none() {
                    first_event = Some(event.clone());
                }

                // Use first user message as preview
                if preview.is_empty() {
                    if let SessionEvent::UserMessage { ref content, .. } = event {
                        preview = content.clone();
                    }
                }
            }
        }

        let first = first_event.ok_or_else(|| {
            crate::error::ClosedCodeError::SessionError("Empty session file".into())
        })?;

        match first {
            SessionEvent::SessionStart {
                session_id,
                model,
                mode,
                working_directory,
                timestamp,
                ..
            } => Ok(SessionMeta {
                session_id,
                model,
                mode,
                working_directory,
                started_at: timestamp,
                last_active: last_timestamp.unwrap_or(timestamp),
                preview,
            }),
            _ => Err(crate::error::ClosedCodeError::SessionError(
                "Session file does not start with SessionStart event".into(),
            )),
        }
    }

    /// Delete a session file.
    pub fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Err(crate::error::ClosedCodeError::SessionNotFound(
                session_id.as_str(),
            ));
        }
        std::fs::remove_file(&path).map_err(crate::error::ClosedCodeError::Io)
    }

    /// Fork a session by copying its file.
    pub fn fork_session(&self, source: &SessionId, target: &SessionId) -> Result<()> {
        let source_path = self.session_path(source);
        if !source_path.exists() {
            return Err(crate::error::ClosedCodeError::SessionNotFound(
                source.as_str(),
            ));
        }
        let target_path = self.session_path(target);
        std::fs::copy(&source_path, &target_path).map_err(crate::error::ClosedCodeError::Io)?;
        Ok(())
    }

    /// Resolve a short session ID prefix to a full SessionId.
    /// Scans .jsonl filenames in the sessions directory.
    /// Returns error if no match or multiple matches (ambiguous).
    pub fn find_by_prefix(&self, prefix: &str) -> Result<SessionId> {
        self.ensure_dir()?;
        let prefix_lower = prefix.to_lowercase().replace('-', "");
        let mut matches = Vec::new();

        for entry in std::fs::read_dir(&self.sessions_dir)
            .map_err(crate::error::ClosedCodeError::Io)?
        {
            let entry = entry.map_err(crate::error::ClosedCodeError::Io)?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Compare without hyphens for flexible matching
                    let stem_normalized = stem.to_lowercase().replace('-', "");
                    if stem_normalized.starts_with(&prefix_lower) {
                        if let Ok(id) = SessionId::parse(stem) {
                            matches.push(id);
                        }
                    }
                }
            }
        }

        match matches.len() {
            0 => Err(crate::error::ClosedCodeError::SessionNotFound(
                prefix.to_string(),
            )),
            1 => Ok(matches.remove(0)),
            n => Err(crate::error::ClosedCodeError::SessionError(format!(
                "Ambiguous prefix '{}': matches {} sessions",
                prefix, n
            ))),
        }
    }

    /// Reconstruct conversation history from session events.
    /// Starts from the last Compact event if present, maps events to `Vec<Content>`.
    pub fn reconstruct_history(events: &[SessionEvent]) -> Vec<Content> {
        use crate::gemini::types::Part;

        // Find the last compact event index
        let start_idx = events
            .iter()
            .rposition(|e| matches!(e, SessionEvent::Compact { .. }))
            .map(|idx| idx) // Start from the compact event itself
            .unwrap_or(0);

        let mut history = Vec::new();

        for event in &events[start_idx..] {
            match event {
                SessionEvent::Compact { summary, .. } => {
                    // Inject compact summary as a user message
                    history.push(Content::user(&format!(
                        "[Previous conversation summary]: {}",
                        summary
                    )));
                }
                SessionEvent::UserMessage { content, .. } => {
                    history.push(Content::user(content));
                }
                SessionEvent::AssistantMessage { content, .. } => {
                    history.push(Content::model(content));
                }
                SessionEvent::ToolCall { name, args, .. } => {
                    history.push(Content {
                        role: Some("model".into()),
                        parts: vec![Part::FunctionCall {
                            name: name.clone(),
                            args: args.clone(),
                            thought_signature: None,
                        }],
                    });
                }
                SessionEvent::ToolResponse { name, result, .. } => {
                    history.push(Content::function_responses(vec![Part::FunctionResponse {
                        name: name.clone(),
                        response: serde_json::json!({"result": result}),
                    }]));
                }
                SessionEvent::ImageAttached {
                    mime_type, ..
                } => {
                    history.push(Content {
                        role: Some("user".into()),
                        parts: vec![Part::InlineData {
                            mime_type: mime_type.clone(),
                            data: String::new(), // Data not stored in events
                        }],
                    });
                }
                // Skip non-content events
                SessionEvent::SessionStart { .. }
                | SessionEvent::ModeChange { .. }
                | SessionEvent::SessionEnd { .. } => {}
            }
        }

        history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn test_store() -> (SessionStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        (store, dir)
    }

    fn make_session_start(session_id: &SessionId) -> SessionEvent {
        SessionEvent::SessionStart {
            session_id: session_id.clone(),
            model: "gemini-2.5-pro".into(),
            mode: "auto".into(),
            working_directory: "/tmp/project".into(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn new_and_default_dir() {
        let store = SessionStore::new(PathBuf::from("/tmp/test-sessions"));
        assert_eq!(
            store.session_path(&SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap()),
            PathBuf::from("/tmp/test-sessions/00000000-0000-0000-0000-000000000001.jsonl")
        );

        let default = SessionStore::default_dir();
        assert!(default.to_string_lossy().contains("sessions"));
    }

    #[test]
    fn ensure_dir_creates_directory() {
        let dir = TempDir::new().unwrap();
        let sessions_dir = dir.path().join("nested").join("sessions");
        let store = SessionStore::new(sessions_dir.clone());
        store.ensure_dir().unwrap();
        assert!(sessions_dir.exists());
    }

    #[test]
    fn save_and_load_events() {
        let (store, _dir) = test_store();
        let id = SessionId::new();

        let start = make_session_start(&id);
        let msg = SessionEvent::UserMessage {
            content: "Hello".into(),
            timestamp: Utc::now(),
        };
        let reply = SessionEvent::AssistantMessage {
            content: "Hi there!".into(),
            timestamp: Utc::now(),
        };

        store.save_event(&id, &start).unwrap();
        store.save_event(&id, &msg).unwrap();
        store.save_event(&id, &reply).unwrap();

        let events = store.load_events(&id).unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], SessionEvent::SessionStart { .. }));
        assert!(matches!(events[1], SessionEvent::UserMessage { .. }));
        assert!(matches!(events[2], SessionEvent::AssistantMessage { .. }));
    }

    #[test]
    fn load_events_empty_file() {
        let (store, _dir) = test_store();
        let id = SessionId::new();

        // Create empty file
        let path = store.session_path(&id);
        store.ensure_dir().unwrap();
        std::fs::write(&path, "").unwrap();

        let events = store.load_events(&id).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn load_events_skips_malformed_lines() {
        let (store, _dir) = test_store();
        let id = SessionId::new();

        let start = make_session_start(&id);
        store.save_event(&id, &start).unwrap();

        // Append a malformed line
        let path = store.session_path(&id);
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{invalid json}}").unwrap();

        let msg = SessionEvent::UserMessage {
            content: "after bad line".into(),
            timestamp: Utc::now(),
        };
        store.save_event(&id, &msg).unwrap();

        let events = store.load_events(&id).unwrap();
        assert_eq!(events.len(), 2); // start + user msg, skipped malformed
    }

    #[test]
    fn load_events_not_found() {
        let (store, _dir) = test_store();
        let id = SessionId::new();
        let result = store.load_events(&id);
        assert!(result.is_err());
    }

    #[test]
    fn list_sessions_empty() {
        let (store, _dir) = test_store();
        let sessions = store.list_sessions().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_multiple_sorted() {
        let (store, _dir) = test_store();

        let id1 = SessionId::new();
        let id2 = SessionId::new();

        // Session 1 — older
        store.save_event(&id1, &make_session_start(&id1)).unwrap();
        store
            .save_event(
                &id1,
                &SessionEvent::UserMessage {
                    content: "First session".into(),
                    timestamp: Utc::now(),
                },
            )
            .unwrap();

        // Session 2 — newer (saved after)
        store.save_event(&id2, &make_session_start(&id2)).unwrap();
        store
            .save_event(
                &id2,
                &SessionEvent::UserMessage {
                    content: "Second session".into(),
                    timestamp: Utc::now(),
                },
            )
            .unwrap();

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        // Most recent first
        assert_eq!(sessions[0].session_id, id2);
        assert_eq!(sessions[0].preview, "Second session");
        assert_eq!(sessions[1].session_id, id1);
        assert_eq!(sessions[1].preview, "First session");
    }

    #[test]
    fn delete_session_exists() {
        let (store, _dir) = test_store();
        let id = SessionId::new();
        store.save_event(&id, &make_session_start(&id)).unwrap();

        assert!(store.session_path(&id).exists());
        store.delete_session(&id).unwrap();
        assert!(!store.session_path(&id).exists());
    }

    #[test]
    fn delete_session_not_found() {
        let (store, _dir) = test_store();
        let id = SessionId::new();
        let result = store.delete_session(&id);
        assert!(result.is_err());
    }

    #[test]
    fn fork_session_success() {
        let (store, _dir) = test_store();
        let source = SessionId::new();
        let target = SessionId::new();

        store
            .save_event(&source, &make_session_start(&source))
            .unwrap();
        store
            .save_event(
                &source,
                &SessionEvent::UserMessage {
                    content: "hello".into(),
                    timestamp: Utc::now(),
                },
            )
            .unwrap();

        store.fork_session(&source, &target).unwrap();

        let source_events = store.load_events(&source).unwrap();
        let target_events = store.load_events(&target).unwrap();
        assert_eq!(source_events.len(), target_events.len());
    }

    #[test]
    fn fork_session_source_not_found() {
        let (store, _dir) = test_store();
        let source = SessionId::new();
        let target = SessionId::new();
        let result = store.fork_session(&source, &target);
        assert!(result.is_err());
    }

    #[test]
    fn reconstruct_history_basic() {
        let events = vec![
            SessionEvent::SessionStart {
                session_id: SessionId::new(),
                model: "test".into(),
                mode: "auto".into(),
                working_directory: "/tmp".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::UserMessage {
                content: "Hello".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::AssistantMessage {
                content: "Hi!".into(),
                timestamp: Utc::now(),
            },
        ];

        let history = SessionStore::reconstruct_history(&events);
        assert_eq!(history.len(), 2); // user + assistant, SessionStart skipped
        assert_eq!(history[0].role.as_deref(), Some("user"));
        assert_eq!(history[1].role.as_deref(), Some("model"));
    }

    #[test]
    fn reconstruct_history_with_compact() {
        let events = vec![
            SessionEvent::SessionStart {
                session_id: SessionId::new(),
                model: "test".into(),
                mode: "auto".into(),
                working_directory: "/tmp".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::UserMessage {
                content: "Old message".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::AssistantMessage {
                content: "Old reply".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::Compact {
                summary: "User discussed database schema.".into(),
                turns_before: 20,
                turns_after: 6,
                timestamp: Utc::now(),
            },
            SessionEvent::UserMessage {
                content: "Recent question".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::AssistantMessage {
                content: "Recent answer".into(),
                timestamp: Utc::now(),
            },
        ];

        let history = SessionStore::reconstruct_history(&events);
        // Should start from Compact: summary + 2 recent messages = 3
        assert_eq!(history.len(), 3);
        // First should be compact summary
        if let Some(crate::gemini::types::Part::Text(ref t)) = history[0].parts.first() {
            assert!(t.contains("database schema"));
        } else {
            panic!("Expected text part for compact summary");
        }
    }

    #[test]
    fn reconstruct_history_with_tool_calls() {
        let events = vec![
            SessionEvent::UserMessage {
                content: "Read file".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::ToolCall {
                name: "read_file".into(),
                args: serde_json::json!({"path": "/tmp/test.rs"}),
                timestamp: Utc::now(),
            },
            SessionEvent::ToolResponse {
                name: "read_file".into(),
                result: "fn main() {}".into(),
                timestamp: Utc::now(),
            },
            SessionEvent::AssistantMessage {
                content: "Here is the file.".into(),
                timestamp: Utc::now(),
            },
        ];

        let history = SessionStore::reconstruct_history(&events);
        assert_eq!(history.len(), 4);
        // Check tool call
        assert!(matches!(
            &history[1].parts[0],
            crate::gemini::types::Part::FunctionCall { name, .. } if name == "read_file"
        ));
        // Check tool response
        assert!(matches!(
            &history[2].parts[0],
            crate::gemini::types::Part::FunctionResponse { name, .. } if name == "read_file"
        ));
    }

    #[test]
    fn reconstruct_history_empty() {
        let history = SessionStore::reconstruct_history(&[]);
        assert!(history.is_empty());
    }

    #[test]
    fn session_path_format() {
        let store = SessionStore::new(PathBuf::from("/data/sessions"));
        let id = SessionId::parse("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap();
        let path = store.session_path(&id);
        assert_eq!(
            path,
            PathBuf::from("/data/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890.jsonl")
        );
    }

    #[test]
    fn reconstruct_history_image_attached() {
        let events = vec![
            SessionEvent::ImageAttached {
                mime_type: "image/png".into(),
                size_bytes: 1024,
                timestamp: Utc::now(),
            },
            SessionEvent::UserMessage {
                content: "What is in this image?".into(),
                timestamp: Utc::now(),
            },
        ];

        let history = SessionStore::reconstruct_history(&events);
        assert_eq!(history.len(), 2);
        assert!(matches!(
            &history[0].parts[0],
            crate::gemini::types::Part::InlineData { mime_type, .. } if mime_type == "image/png"
        ));
    }

    #[test]
    fn find_by_prefix_single_match() {
        let (store, _dir) = test_store();
        let id = SessionId::new();
        store.save_event(&id, &make_session_start(&id)).unwrap();

        let prefix = &id.as_str()[..8];
        let found = store.find_by_prefix(prefix).unwrap();
        assert_eq!(found, id);
    }

    #[test]
    fn find_by_prefix_full_uuid() {
        let (store, _dir) = test_store();
        let id = SessionId::new();
        store.save_event(&id, &make_session_start(&id)).unwrap();

        let found = store.find_by_prefix(&id.as_str()).unwrap();
        assert_eq!(found, id);
    }

    #[test]
    fn find_by_prefix_no_match() {
        let (store, _dir) = test_store();
        let result = store.find_by_prefix("nonexist");
        assert!(result.is_err());
    }

    #[test]
    fn find_by_prefix_ambiguous() {
        let (store, _dir) = test_store();

        // Create two sessions — use a single-char prefix that both will match
        let id1 = SessionId::new();
        let id2 = SessionId::new();
        store.save_event(&id1, &make_session_start(&id1)).unwrap();
        store.save_event(&id2, &make_session_start(&id2)).unwrap();

        // Empty prefix matches all
        let result = store.find_by_prefix("");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Ambiguous") || err_msg.contains("matches 2"));
    }
}

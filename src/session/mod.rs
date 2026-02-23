pub mod store;
pub mod transcript;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use uuid::Uuid;

// ── SessionId ──

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }

    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── SessionEvent ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionStart {
        session_id: SessionId,
        model: String,
        mode: String,
        working_directory: String,
        timestamp: DateTime<Utc>,
    },
    UserMessage {
        content: String,
        timestamp: DateTime<Utc>,
    },
    AssistantMessage {
        content: String,
        timestamp: DateTime<Utc>,
    },
    ToolCall {
        name: String,
        args: Value,
        timestamp: DateTime<Utc>,
    },
    ToolResponse {
        name: String,
        result: String,
        timestamp: DateTime<Utc>,
    },
    ModeChange {
        from: String,
        to: String,
        timestamp: DateTime<Utc>,
    },
    Compact {
        summary: String,
        turns_before: usize,
        turns_after: usize,
        timestamp: DateTime<Utc>,
    },
    ImageAttached {
        mime_type: String,
        size_bytes: u64,
        timestamp: DateTime<Utc>,
    },
    SessionEnd {
        timestamp: DateTime<Utc>,
    },
}

impl SessionEvent {
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::SessionStart { timestamp, .. } => *timestamp,
            Self::UserMessage { timestamp, .. } => *timestamp,
            Self::AssistantMessage { timestamp, .. } => *timestamp,
            Self::ToolCall { timestamp, .. } => *timestamp,
            Self::ToolResponse { timestamp, .. } => *timestamp,
            Self::ModeChange { timestamp, .. } => *timestamp,
            Self::Compact { timestamp, .. } => *timestamp,
            Self::ImageAttached { timestamp, .. } => *timestamp,
            Self::SessionEnd { timestamp } => *timestamp,
        }
    }
}

// ── SessionMeta ──

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub session_id: SessionId,
    pub model: String,
    pub mode: String,
    pub working_directory: String,
    pub started_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub preview: String,
}

impl SessionMeta {
    /// Human-friendly relative time string (e.g., "5 minutes ago").
    pub fn relative_time(&self) -> String {
        let now = Utc::now();
        let duration = now.signed_duration_since(self.last_active);

        let total_seconds = duration.num_seconds();
        if total_seconds < 60 {
            return "just now".to_string();
        }

        let minutes = duration.num_minutes();
        if minutes < 60 {
            return format!(
                "{} minute{} ago",
                minutes,
                if minutes == 1 { "" } else { "s" }
            );
        }

        let hours = duration.num_hours();
        if hours < 24 {
            return format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" });
        }

        let days = duration.num_days();
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    }

    /// Truncate preview to at most 80 characters.
    pub fn truncated_preview(&self) -> String {
        if self.preview.len() <= 80 {
            self.preview.clone()
        } else {
            format!("{}...", &self.preview[..77])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn session_id_new_is_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn session_id_parse_valid() {
        let id = SessionId::new();
        let s = id.as_str();
        let parsed = SessionId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn session_id_parse_invalid() {
        assert!(SessionId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn session_id_display_roundtrip() {
        let id = SessionId::new();
        let display = format!("{}", id);
        let parsed = SessionId::parse(&display).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn session_id_default() {
        let a = SessionId::default();
        let b = SessionId::default();
        assert_ne!(a, b);
    }

    #[test]
    fn session_event_serde_session_start() {
        let ts = Utc::now();
        let event = SessionEvent::SessionStart {
            session_id: SessionId::new(),
            model: "gemini-2.5-pro".into(),
            mode: "auto".into(),
            working_directory: "/home/user/project".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_user_message() {
        let event = SessionEvent::UserMessage {
            content: "Hello, world!".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_assistant_message() {
        let event = SessionEvent::AssistantMessage {
            content: "I can help with that.".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_tool_call() {
        let event = SessionEvent::ToolCall {
            name: "read_file".into(),
            args: serde_json::json!({"path": "/tmp/test.rs"}),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_tool_response() {
        let event = SessionEvent::ToolResponse {
            name: "read_file".into(),
            result: "file contents here".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_mode_change() {
        let event = SessionEvent::ModeChange {
            from: "explore".into(),
            to: "execute".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_compact() {
        let event = SessionEvent::Compact {
            summary: "User asked about database schema.".into(),
            turns_before: 20,
            turns_after: 6,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_image_attached() {
        let event = SessionEvent::ImageAttached {
            mime_type: "image/png".into(),
            size_bytes: 54321,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_serde_session_end() {
        let event = SessionEvent::SessionEnd {
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn session_event_timestamp_accessor() {
        let ts = Utc::now();
        let event = SessionEvent::UserMessage {
            content: "test".into(),
            timestamp: ts,
        };
        assert_eq!(event.timestamp(), ts);
    }

    #[test]
    fn session_event_tagged_json_format() {
        let event = SessionEvent::UserMessage {
            content: "hello".into(),
            timestamp: Utc::now(),
        };
        let json: Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "user_message");
        assert_eq!(json["content"], "hello");
    }

    #[test]
    fn session_meta_relative_time_minutes() {
        let meta = SessionMeta {
            session_id: SessionId::new(),
            model: "gemini-2.5-pro".into(),
            mode: "auto".into(),
            working_directory: "/tmp".into(),
            started_at: Utc::now(),
            last_active: Utc::now() - Duration::minutes(5),
            preview: "test".into(),
        };
        assert_eq!(meta.relative_time(), "5 minutes ago");
    }

    #[test]
    fn session_meta_relative_time_hours() {
        let meta = SessionMeta {
            session_id: SessionId::new(),
            model: "gemini-2.5-pro".into(),
            mode: "auto".into(),
            working_directory: "/tmp".into(),
            started_at: Utc::now(),
            last_active: Utc::now() - Duration::hours(3),
            preview: "test".into(),
        };
        assert_eq!(meta.relative_time(), "3 hours ago");
    }

    #[test]
    fn session_meta_relative_time_days() {
        let meta = SessionMeta {
            session_id: SessionId::new(),
            model: "gemini-2.5-pro".into(),
            mode: "auto".into(),
            working_directory: "/tmp".into(),
            started_at: Utc::now(),
            last_active: Utc::now() - Duration::days(2),
            preview: "test".into(),
        };
        assert_eq!(meta.relative_time(), "2 days ago");
    }

    #[test]
    fn session_meta_truncated_preview_short() {
        let meta = SessionMeta {
            session_id: SessionId::new(),
            model: "test".into(),
            mode: "auto".into(),
            working_directory: "/tmp".into(),
            started_at: Utc::now(),
            last_active: Utc::now(),
            preview: "Short preview".into(),
        };
        assert_eq!(meta.truncated_preview(), "Short preview");
    }

    #[test]
    fn session_meta_truncated_preview_long() {
        let long = "a".repeat(100);
        let meta = SessionMeta {
            session_id: SessionId::new(),
            model: "test".into(),
            mode: "auto".into(),
            working_directory: "/tmp".into(),
            started_at: Utc::now(),
            last_active: Utc::now(),
            preview: long,
        };
        let truncated = meta.truncated_preview();
        assert_eq!(truncated.len(), 80);
        assert!(truncated.ends_with("..."));
    }
}

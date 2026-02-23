# Phase 8a: Session Management + Context Management

**Goal**: Persistent sessions with JSONL storage, enabling resume, fork, compact, and transcript export. Conversations survive across CLI invocations and can be replayed, branched, and summarized.

**Depends on**: Phase 5 (Configuration + Enhanced REPL)

---

## Phase Dependency Graph (within Phase 8a)

```
8a.1 Session Module Foundation (Session, SessionEvent, SessionMeta)
  │
  └──► 8a.2 SessionStore (JSONL persistence: save, load, list, delete)
              │
              ├──► 8a.3 Orchestrator Integration (session_id, emit_event hooks)
              │         │
              │         └──► 8a.4 History Reconstruction + Fork + Compact
              │
              ├──► 8a.5 Config + CLI Changes ([session], resume subcommand)
              │         │
              │         ▼
              └──► 8a.6 Slash Commands (/resume, /new, /fork, /compact, /history, /export)
                        │
                        └──► 8a.7 TranscriptWriter (optional markdown export)
```

---

## Files Overview

```
src/
  session/
    mod.rs             # NEW: Session, SessionId, SessionEvent, SessionMeta types
    store.rs           # NEW: SessionStore — JSONL persistence (save, load, list, delete, fork)
    transcript.rs      # NEW: TranscriptWriter — optional markdown export
  agent/
    orchestrator.rs    # MODIFIED: Add session_id, session_store, emit_event hooks
  config.rs            # MODIFIED: Add SessionConfig, [session] TOML section
  cli.rs               # MODIFIED: Add Resume subcommand
  error.rs             # MODIFIED: Add SessionNotFound, SessionError variants
  repl.rs              # MODIFIED: 6 new slash commands, startup resume, session lifecycle
  lib.rs               # MODIFIED: Add pub mod session;
  main.rs              # MODIFIED: Resume subcommand dispatch
```

### New Cargo Dependencies

```toml
[dependencies]
chrono = { version = "0.4", features = ["serde"] }
```

`uuid` is already present in the project. `serde` and `serde_json` are already present. `chrono` adds timestamp support with serde serialization.

---

## Sub-Phase 8a.1: Session Module Foundation

### New File: `src/session/mod.rs`

Module root with core types: `SessionId`, `SessionEvent`, `SessionMeta`.

```rust
pub mod store;
pub mod transcript;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use std::fmt;
```

**SessionId — UUID wrapper:**

```rust
/// A unique session identifier wrapping a UUID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Create a new random session ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parse a session ID from a string.
    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }

    /// Get the UUID as a string.
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
```

**SessionEvent — serde-tagged enum for JSONL lines:**

```rust
/// A single event in a session's JSONL file.
///
/// Each event is one JSON line. Events are append-only and ordered
/// chronologically. The full session can be reconstructed by replaying events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    /// First event in every session file.
    SessionStart {
        session_id: SessionId,
        model: String,
        mode: String,
        working_directory: String,
        timestamp: DateTime<Utc>,
    },

    /// User sent a message.
    UserMessage {
        content: String,
        timestamp: DateTime<Utc>,
    },

    /// Assistant (model) responded with text.
    AssistantMessage {
        content: String,
        timestamp: DateTime<Utc>,
    },

    /// A tool was called by the model.
    ToolCall {
        name: String,
        args: Value,
        timestamp: DateTime<Utc>,
    },

    /// A tool returned a result.
    ToolResponse {
        name: String,
        result: Value,
        timestamp: DateTime<Utc>,
    },

    /// The user changed modes.
    ModeChange {
        from: String,
        to: String,
        timestamp: DateTime<Utc>,
    },

    /// The conversation was compacted (summarized).
    Compact {
        summary: String,
        turns_before: usize,
        turns_after: usize,
        timestamp: DateTime<Utc>,
    },

    /// An image was described and injected into context.
    ImageAttached {
        source: String,
        description: String,
        timestamp: DateTime<Utc>,
    },

    /// Last event — session ended normally.
    SessionEnd {
        timestamp: DateTime<Utc>,
    },
}

impl SessionEvent {
    /// Get the timestamp of this event.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::SessionStart { timestamp, .. }
            | Self::UserMessage { timestamp, .. }
            | Self::AssistantMessage { timestamp, .. }
            | Self::ToolCall { timestamp, .. }
            | Self::ToolResponse { timestamp, .. }
            | Self::ModeChange { timestamp, .. }
            | Self::Compact { timestamp, .. }
            | Self::ImageAttached { timestamp, .. }
            | Self::SessionEnd { timestamp, .. } => *timestamp,
        }
    }
}
```

**SessionMeta — lightweight metadata for listing sessions:**

```rust
/// Lightweight metadata about a session, parsed from only the first and
/// last lines of a JSONL file. Used by `SessionStore::list_sessions()` to
/// avoid loading entire session histories.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// Unique session identifier.
    pub session_id: SessionId,
    /// Model used when the session started.
    pub model: String,
    /// Mode when the session started.
    pub mode: String,
    /// Working directory when the session started.
    pub working_directory: String,
    /// Timestamp of the first event (SessionStart).
    pub started_at: DateTime<Utc>,
    /// Timestamp of the most recent event.
    pub last_event_at: DateTime<Utc>,
    /// Total number of events in the JSONL file.
    pub event_count: usize,
    /// Preview text: first user message (truncated to 80 chars).
    pub preview: Option<String>,
}

impl SessionMeta {
    /// Human-readable relative time since the session started (e.g., "2h ago", "3d ago").
    pub fn relative_time(&self) -> String {
        let duration = Utc::now() - self.last_event_at;
        if duration.num_minutes() < 1 {
            "just now".to_string()
        } else if duration.num_minutes() < 60 {
            format!("{}m ago", duration.num_minutes())
        } else if duration.num_hours() < 24 {
            format!("{}h ago", duration.num_hours())
        } else {
            format!("{}d ago", duration.num_days())
        }
    }
}
```

**JSONL serialization format examples:**

```jsonl
{"type":"session_start","session_id":"a1b2c3d4-...","model":"gemini-3.1-pro-preview","mode":"explore","working_directory":"/Users/me/project","timestamp":"2026-02-23T10:00:00Z"}
{"type":"user_message","content":"What files are in this project?","timestamp":"2026-02-23T10:00:05Z"}
{"type":"tool_call","name":"list_directory","args":{"path":"."},"timestamp":"2026-02-23T10:00:06Z"}
{"type":"tool_response","name":"list_directory","result":{"files":["Cargo.toml","src/"]},"timestamp":"2026-02-23T10:00:06Z"}
{"type":"assistant_message","content":"Your project contains Cargo.toml and a src/ directory...","timestamp":"2026-02-23T10:00:08Z"}
{"type":"mode_change","from":"explore","to":"plan","timestamp":"2026-02-23T10:05:00Z"}
{"type":"compact","summary":"User explored project structure...","turns_before":47,"turns_after":6,"timestamp":"2026-02-23T10:30:00Z"}
{"type":"session_end","timestamp":"2026-02-23T10:45:00Z"}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| JSONL (one JSON per line) | Append-only, crash-safe (partial writes lose at most one event), `grep`/`jq`-friendly, easy to parse incrementally. |
| `#[serde(tag = "type")]` tagged enum | Each line self-identifies. No need for external framing. Clean deserialization with `serde_json::from_str()`. |
| `SessionId` wraps `Uuid` | Type safety — prevents accidentally passing a random string as a session ID. `Display` and `FromStr` for CLI/display. |
| `SessionMeta` is separate from full event list | Listing sessions should be fast. `list_sessions()` reads only the first and last lines of each JSONL file, not the entire history. |
| `ImageAttached` event | Tracks image descriptions injected by Phase 8b. Allows history reconstruction to include image context. |
| Timestamps use `chrono::DateTime<Utc>` | UTC is unambiguous. Chrono provides serde support and human-readable formatting. |
| `relative_time()` on `SessionMeta` | User-facing display: `"2h ago"` is more useful than `"2026-02-23T10:00:00Z"`. |

### `src/lib.rs` — Modified

Add `pub mod session;` to module declarations.

---

## Sub-Phase 8a.2: SessionStore

### New File: `src/session/store.rs`

Handles all JSONL file I/O: saving events, loading events, listing sessions, deleting sessions, and forking.

```rust
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::{Content, Part};

use super::{SessionEvent, SessionId, SessionMeta};
```

**SessionStore struct:**

```rust
/// Persistent storage for session JSONL files.
///
/// Sessions are stored at `~/.closed-code/sessions/<uuid>.jsonl`.
/// Each file contains one `SessionEvent` JSON object per line.
#[derive(Debug, Clone)]
pub struct SessionStore {
    sessions_dir: PathBuf,
}

impl SessionStore {
    /// Create a new SessionStore with the given directory.
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    /// The default sessions directory: `~/.closed-code/sessions/`.
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".closed-code")
            .join("sessions")
    }

    /// Ensure the sessions directory exists.
    pub fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.sessions_dir).map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Failed to create sessions directory {}: {}",
                self.sessions_dir.display(),
                e,
            ))
        })
    }

    /// Path to a session's JSONL file.
    fn session_path(&self, session_id: &SessionId) -> PathBuf {
        self.sessions_dir.join(format!("{}.jsonl", session_id))
    }
}
```

**Saving events (append-only):**

```rust
impl SessionStore {
    /// Append a single event to a session's JSONL file.
    ///
    /// Creates the file if it doesn't exist. Each event is one JSON line
    /// followed by a newline. The file is opened in append mode and flushed
    /// after each write for crash safety.
    pub fn save_event(&self, session_id: &SessionId, event: &SessionEvent) -> Result<()> {
        self.ensure_dir()?;

        let path = self.session_path(session_id);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| {
                ClosedCodeError::SessionError(format!(
                    "Failed to open session file {}: {}",
                    path.display(),
                    e,
                ))
            })?;

        let json = serde_json::to_string(event).map_err(|e| {
            ClosedCodeError::SessionError(format!("Failed to serialize session event: {}", e))
        })?;

        writeln!(file, "{}", json).map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Failed to write to session file {}: {}",
                path.display(),
                e,
            ))
        })?;

        file.flush().map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Failed to flush session file {}: {}",
                path.display(),
                e,
            ))
        })?;

        Ok(())
    }
}
```

**Loading events:**

```rust
impl SessionStore {
    /// Load all events from a session's JSONL file.
    ///
    /// Returns events in chronological order. Skips malformed lines
    /// with a tracing warning (crash recovery: partial writes may leave
    /// an incomplete last line).
    pub fn load_events(&self, session_id: &SessionId) -> Result<Vec<SessionEvent>> {
        let path = self.session_path(session_id);

        if !path.exists() {
            return Err(ClosedCodeError::SessionNotFound(session_id.to_string()));
        }

        let file = fs::File::open(&path).map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Failed to open session file {}: {}",
                path.display(),
                e,
            ))
        })?;

        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line.map_err(|e| {
                ClosedCodeError::SessionError(format!(
                    "Failed to read line {} of {}: {}",
                    line_num + 1,
                    path.display(),
                    e,
                ))
            })?;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<SessionEvent>(trimmed) {
                Ok(event) => events.push(event),
                Err(e) => {
                    tracing::warn!(
                        "Skipping malformed event on line {} of {}: {}",
                        line_num + 1,
                        path.display(),
                        e,
                    );
                }
            }
        }

        Ok(events)
    }
}
```

**Listing sessions:**

```rust
impl SessionStore {
    /// List all sessions with metadata, sorted by most recent first.
    ///
    /// Reads only the first and last lines of each JSONL file to build
    /// `SessionMeta` without loading full histories.
    pub fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        self.ensure_dir()?;

        let mut sessions = Vec::new();

        let entries = fs::read_dir(&self.sessions_dir).map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Failed to read sessions directory {}: {}",
                self.sessions_dir.display(),
                e,
            ))
        })?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            match self.read_session_meta(&path) {
                Ok(meta) => sessions.push(meta),
                Err(e) => {
                    tracing::warn!(
                        "Skipping malformed session file {}: {}",
                        path.display(),
                        e,
                    );
                }
            }
        }

        // Sort by most recent first
        sessions.sort_by(|a, b| b.last_event_at.cmp(&a.last_event_at));

        Ok(sessions)
    }

    /// Read metadata from a session file by parsing the first line
    /// (SessionStart) and counting/scanning for the last line.
    fn read_session_meta(&self, path: &Path) -> Result<SessionMeta> {
        let file = fs::File::open(path).map_err(|e| {
            ClosedCodeError::SessionError(format!("Failed to open {}: {}", path.display(), e))
        })?;

        let reader = BufReader::new(file);
        let mut first_line = None;
        let mut last_line = None;
        let mut event_count = 0;
        let mut preview = None;

        for line in reader.lines() {
            let line = line.map_err(|e| {
                ClosedCodeError::SessionError(format!(
                    "Failed to read {}: {}",
                    path.display(),
                    e,
                ))
            })?;
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }

            event_count += 1;

            if first_line.is_none() {
                first_line = Some(trimmed.clone());
            }

            // Capture first user message as preview
            if preview.is_none() {
                if let Ok(event) = serde_json::from_str::<SessionEvent>(&trimmed) {
                    if let SessionEvent::UserMessage { ref content, .. } = event {
                        let truncated = if content.len() > 80 {
                            format!("{}...", &content[..77])
                        } else {
                            content.clone()
                        };
                        preview = Some(truncated);
                    }
                }
            }

            last_line = Some(trimmed);
        }

        let first = first_line.ok_or_else(|| {
            ClosedCodeError::SessionError(format!("Empty session file: {}", path.display()))
        })?;

        let last = last_line.unwrap_or_else(|| first.clone());

        // Parse SessionStart from first line
        let start_event: SessionEvent = serde_json::from_str(&first).map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Invalid first event in {}: {}",
                path.display(),
                e,
            ))
        })?;

        let (session_id, model, mode, working_directory, started_at) = match start_event {
            SessionEvent::SessionStart {
                session_id,
                model,
                mode,
                working_directory,
                timestamp,
            } => (session_id, model, mode, working_directory, timestamp),
            _ => {
                return Err(ClosedCodeError::SessionError(format!(
                    "First event in {} is not SessionStart",
                    path.display(),
                )));
            }
        };

        // Parse last event for its timestamp
        let last_event: SessionEvent = serde_json::from_str(&last).map_err(|_| {
            ClosedCodeError::SessionError(format!(
                "Invalid last event in {}",
                path.display(),
            ))
        })?;
        let last_event_at = last_event.timestamp();

        Ok(SessionMeta {
            session_id,
            model,
            mode,
            working_directory,
            started_at,
            last_event_at,
            event_count,
            preview,
        })
    }
}
```

**Delete and fork:**

```rust
impl SessionStore {
    /// Delete a session's JSONL file.
    pub fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        let path = self.session_path(session_id);

        if !path.exists() {
            return Err(ClosedCodeError::SessionNotFound(session_id.to_string()));
        }

        fs::remove_file(&path).map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Failed to delete session file {}: {}",
                path.display(),
                e,
            ))
        })
    }

    /// Fork a session by copying its JSONL file to a new session ID.
    ///
    /// The new session file is an exact copy. The caller should then
    /// append a new `SessionStart` event to mark the fork point.
    pub fn fork_session(
        &self,
        source: &SessionId,
        target: &SessionId,
    ) -> Result<()> {
        let source_path = self.session_path(source);
        let target_path = self.session_path(target);

        if !source_path.exists() {
            return Err(ClosedCodeError::SessionNotFound(source.to_string()));
        }

        fs::copy(&source_path, &target_path).map_err(|e| {
            ClosedCodeError::SessionError(format!(
                "Failed to fork session from {} to {}: {}",
                source, target, e,
            ))
        })?;

        Ok(())
    }
}
```

**History reconstruction:**

```rust
impl SessionStore {
    /// Reconstruct `Vec<Content>` from session events.
    ///
    /// Replays events to rebuild the conversation history that can be
    /// passed to the Gemini API. If a `Compact` event is found, only
    /// events after the last compact are used (the compact summary
    /// becomes the first user message).
    pub fn reconstruct_history(events: &[SessionEvent]) -> Vec<Content> {
        // Find the last Compact event index, if any
        let start_index = events
            .iter()
            .rposition(|e| matches!(e, SessionEvent::Compact { .. }))
            .unwrap_or(0);

        let mut history = Vec::new();

        for event in &events[start_index..] {
            match event {
                SessionEvent::Compact { summary, .. } => {
                    // Compact summary becomes the first context message
                    history.push(Content::user(&format!(
                        "[Previous conversation summary]\n{}",
                        summary,
                    )));
                }
                SessionEvent::UserMessage { content, .. } => {
                    history.push(Content::user(content));
                }
                SessionEvent::AssistantMessage { content, .. } => {
                    history.push(Content::model(content));
                }
                SessionEvent::ToolCall { name, args, .. } => {
                    // Tool calls are model messages with FunctionCall parts
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
                    // Tool responses are user messages with FunctionResponse parts
                    history.push(Content::function_responses(vec![
                        Part::FunctionResponse {
                            name: name.clone(),
                            response: result.clone(),
                        },
                    ]));
                }
                SessionEvent::ImageAttached { description, .. } => {
                    history.push(Content::user(description));
                }
                // SessionStart, SessionEnd, ModeChange don't contribute to API history
                _ => {}
            }
        }

        history
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| Append-only JSONL | Crash-safe: if the process dies mid-write, at most one line is lost. No corruption of previous data. |
| `save_event()` flushes after each write | Ensures data is written to disk. Latency is negligible for text-sized events. |
| Malformed lines are skipped with warning | Graceful recovery from crashes or partial writes. The session is still usable. |
| `list_sessions()` reads first+last lines only | Fast listing even with large session files. Avoids loading megabytes of tool call data. |
| `reconstruct_history()` starts from last Compact | After a compact, earlier events are irrelevant. The compact summary replaces them. |
| Fork copies the entire file | Simple, atomic. The fork inherits the full history. Caller appends a new SessionStart marker. |
| `Content::function_responses()` for ToolResponse | Matches the Gemini API convention where tool responses are sent as user messages with FunctionResponse parts. |

---

## Sub-Phase 8a.3: Orchestrator Integration

### `src/agent/orchestrator.rs` — Modified

**New fields:**

```rust
use crate::session::{SessionEvent, SessionId};
use crate::session::store::SessionStore;

pub struct Orchestrator {
    // ... existing 17 fields ...

    // Phase 8a
    session_id: Option<SessionId>,
    session_store: Option<SessionStore>,
}
```

**Updated constructor:**

```rust
impl Orchestrator {
    pub fn new(
        client: Arc<GeminiClient>,
        mode: Mode,
        working_directory: PathBuf,
        max_output_tokens: u32,
        approval_handler: Arc<dyn ApprovalHandler>,
        personality: Personality,
        context_window_turns: usize,
        sandbox: Arc<dyn Sandbox>,
        protected_paths: Vec<String>,
    ) -> Self {
        // ... existing initialization ...

        Self {
            // ... existing fields ...
            session_id: None,
            session_store: None,
        }
    }
}
```

**New session methods:**

```rust
impl Orchestrator {
    /// Set the session ID and store for this orchestrator.
    /// Called during REPL startup or when resuming a session.
    pub fn set_session(&mut self, id: SessionId, store: SessionStore) {
        self.session_id = Some(id);
        self.session_store = Some(store);
    }

    /// Get the current session ID, if any.
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// Get a reference to the session store, if configured.
    pub fn session_store(&self) -> Option<&SessionStore> {
        self.session_store.as_ref()
    }

    /// Emit a session event. Fire-and-forget: logs warnings on failure
    /// but never returns an error (session persistence must not break
    /// the conversation flow).
    fn emit_event(&self, event: SessionEvent) {
        if let (Some(session_id), Some(store)) = (&self.session_id, &self.session_store) {
            if let Err(e) = store.save_event(session_id, &event) {
                tracing::warn!("Failed to save session event: {}", e);
            }
        }
    }

    /// Replace the current history with a reconstructed one (for resume/compact).
    pub fn set_history(&mut self, history: Vec<Content>) {
        self.history = history;
    }

    /// Get a clone of the current history (for compact summarization).
    pub fn history(&self) -> &[Content] {
        &self.history
    }

    /// Start a new session, emitting a SessionStart event.
    pub fn start_session(&mut self, store: SessionStore) {
        let session_id = SessionId::new();
        self.emit_session_start(&session_id);
        self.session_id = Some(session_id);
        self.session_store = Some(store);
    }

    fn emit_session_start(&self, session_id: &SessionId) {
        if let Some(store) = &self.session_store {
            let event = SessionEvent::SessionStart {
                session_id: session_id.clone(),
                model: self.model_name.clone(),
                mode: self.mode.to_string(),
                working_directory: self.working_directory.display().to_string(),
                timestamp: Utc::now(),
            };
            if let Err(e) = store.save_event(session_id, &event) {
                tracing::warn!("Failed to save SessionStart event: {}", e);
            }
        }
    }
}
```

**Modified `handle_user_input_streaming()`:**

Event emission hooks are added at key points in the existing method:

```rust
// After pushing user message to history (existing line ~108):
self.history.push(Content::user(input));
// NEW: Emit user message event
self.emit_event(SessionEvent::UserMessage {
    content: input.to_string(),
    timestamp: Utc::now(),
});

// After pushing model response to history (existing line ~167):
self.history.push(Content::model(&text));
// NEW: Emit assistant message event
self.emit_event(SessionEvent::AssistantMessage {
    content: text.clone(),
    timestamp: Utc::now(),
});

// After executing each tool call (inside the function call loop):
// NEW: Emit tool call event
self.emit_event(SessionEvent::ToolCall {
    name: name.clone(),
    args: args.clone(),
    timestamp: Utc::now(),
});

// After getting tool result:
// NEW: Emit tool response event
self.emit_event(SessionEvent::ToolResponse {
    name: name.clone(),
    result: result.clone(),
    timestamp: Utc::now(),
});
```

**Modified `set_mode()`:**

```rust
pub fn set_mode(&mut self, mode: Mode) {
    let old_mode = self.mode;
    // ... existing mode switch logic ...
    self.mode = mode;

    // NEW: Emit mode change event
    self.emit_event(SessionEvent::ModeChange {
        from: old_mode.to_string(),
        to: mode.to_string(),
        timestamp: Utc::now(),
    });
}
```

**Modified `clear_history()`:**

```rust
pub fn clear_history(&mut self) {
    // Emit session end for the old session
    self.emit_event(SessionEvent::SessionEnd {
        timestamp: Utc::now(),
    });

    self.history.clear();

    // Start a new session
    if self.session_store.is_some() {
        let new_id = SessionId::new();
        self.emit_session_start(&new_id);
        self.session_id = Some(new_id);
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| `session_id: Option<SessionId>` | Optional — tests and one-shot mode work unchanged without sessions. |
| `emit_event()` is fire-and-forget | Session persistence must NEVER crash the conversation. A failed save is logged but the conversation continues. |
| Events emitted inline in `handle_user_input_streaming()` | Minimal code change — 4 emit calls inserted at existing points. No new method needed. |
| `set_history()` public method | Needed for resume (load history) and compact (replace history). |
| `clear_history()` emits SessionEnd + starts new | `/clear` effectively ends the current session and starts fresh. The old session is preserved on disk. |

---

## Sub-Phase 8a.4: History Reconstruction + Fork + Compact

### Orchestrator methods for fork and compact

```rust
impl Orchestrator {
    /// Fork the current session into a new session ID.
    ///
    /// Copies the JSONL file, switches to the new session ID.
    pub fn fork_session(&mut self) -> Result<Option<SessionId>> {
        let (session_id, store) = match (&self.session_id, &self.session_store) {
            (Some(id), Some(store)) => (id.clone(), store.clone()),
            _ => return Ok(None),
        };

        let new_id = SessionId::new();
        store.fork_session(&session_id, &new_id)?;

        // Mark the fork point in the new session
        store.save_event(&new_id, &SessionEvent::SessionStart {
            session_id: new_id.clone(),
            model: self.model_name.clone(),
            mode: self.mode.to_string(),
            working_directory: self.working_directory.display().to_string(),
            timestamp: Utc::now(),
        })?;

        self.session_id = Some(new_id.clone());
        Ok(Some(new_id))
    }

    /// Compact the conversation by sending it to the LLM for summarization.
    ///
    /// Replaces the history with: compact summary + last N turns.
    /// Returns the summary text on success.
    pub async fn compact_history(&mut self, keep_recent: usize) -> Result<String> {
        if self.history.len() <= keep_recent + 1 {
            return Err(ClosedCodeError::SessionError(
                "History is too short to compact.".into(),
            ));
        }

        let turns_before = self.history.len();

        // Build a summary prompt from the current history
        let history_text: String = self
            .history
            .iter()
            .filter_map(|content| {
                let role = content.role.as_deref().unwrap_or("system");
                let text: String = content
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if text.is_empty() {
                    None
                } else {
                    Some(format!("[{}]: {}", role, text))
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let summary_prompt = format!(
            "Summarize this conversation in 500 words or fewer. \
             Preserve key decisions, code changes, file paths mentioned, \
             and any important context. Be concise but thorough.\n\n{}",
            history_text,
        );

        // Use a one-shot API call for summarization
        let request = crate::gemini::types::GenerateContentRequest {
            contents: vec![Content::user(&summary_prompt)],
            system_instruction: Some(Content::system(
                "You are a conversation summarizer. Produce a concise summary \
                 that captures the essential context needed to continue the conversation."
            )),
            generation_config: Some(crate::gemini::types::GenerationConfig {
                temperature: Some(0.3),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(2048),
            }),
            tools: None,
            tool_config: None,
        };

        let response = self.client.generate_content(&request).await?;
        let summary = response.text().unwrap_or("").to_string();

        if summary.is_empty() {
            return Err(ClosedCodeError::SessionError(
                "Compact summarization returned empty response.".into(),
            ));
        }

        // Keep the last N turns
        let recent_start = self.history.len().saturating_sub(keep_recent);
        let recent_turns: Vec<Content> = self.history[recent_start..].to_vec();

        // Replace history: summary + recent turns
        self.history.clear();
        self.history.push(Content::user(&format!(
            "[Previous conversation summary]\n{}",
            summary,
        )));
        self.history.extend(recent_turns);

        let turns_after = self.history.len();

        // Emit compact event
        self.emit_event(SessionEvent::Compact {
            summary: summary.clone(),
            turns_before,
            turns_after,
            timestamp: Utc::now(),
        });

        Ok(summary)
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| Compact keeps last N turns | Recent context is most relevant. The summary covers older turns. Default N=5 (configurable from slash command). |
| Summary done via one-shot API call | Avoids interference with the main conversation. Uses a dedicated system prompt for summarization. |
| Low temperature (0.3) for summarization | Accuracy over creativity. The summary should faithfully capture what happened. |
| Fork copies JSONL then appends new SessionStart | The fork has full history. The new SessionStart marks where the fork diverged. |
| `reconstruct_history()` starts from last Compact | Efficient resume — doesn't replay the entire session, just events after the most recent compact. |

---

## Sub-Phase 8a.5: Config + CLI Changes

### `src/config.rs` — Modified

**New `SessionConfig` struct:**

```rust
#[derive(Debug, Default, Deserialize)]
pub struct SessionConfig {
    /// Enable automatic session saving. Default: true.
    pub auto_save: Option<bool>,
    /// Enable markdown transcript logging. Default: false.
    pub transcript_logging: Option<bool>,
    /// Custom sessions directory (overrides default ~/.closed-code/sessions/).
    pub sessions_dir: Option<String>,
}
```

**Updated `TomlConfig`:**

```rust
#[derive(Debug, Default, Deserialize)]
pub struct TomlConfig {
    // ... existing fields ...
    #[serde(default)]
    pub session: Option<SessionConfig>,
}
```

**Updated `Config` struct:**

```rust
#[derive(Debug, Clone)]
pub struct Config {
    // ... existing fields ...
    pub session_auto_save: bool,
    pub session_transcript_logging: bool,
    pub sessions_dir: PathBuf,
}
```

**Updated `Config::from_cli()`:**

```rust
let session_auto_save = merged
    .session
    .as_ref()
    .and_then(|s| s.auto_save)
    .unwrap_or(true);

let session_transcript_logging = merged
    .session
    .as_ref()
    .and_then(|s| s.transcript_logging)
    .unwrap_or(false);

let sessions_dir = merged
    .session
    .as_ref()
    .and_then(|s| s.sessions_dir.as_ref())
    .map(PathBuf::from)
    .unwrap_or_else(|| crate::session::store::SessionStore::default_dir());
```

**Updated `Config::merge()`:**

```rust
fn merge(base: TomlConfig, overlay: TomlConfig) -> TomlConfig {
    TomlConfig {
        // ... existing merges ...
        session: overlay.session.or(base.session),
    }
}
```

**Example `config.toml`:**

```toml
model = "gemini-3.1-pro-preview"
default_mode = "explore"

[session]
auto_save = true
transcript_logging = false
# sessions_dir = "/custom/path/sessions"  # Optional override
```

### `src/cli.rs` — Modified

**Updated `Commands` enum:**

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Send a one-shot question (non-interactive)
    Ask {
        /// The question to ask
        question: String,
    },

    /// Resume a previous session
    Resume {
        /// Session ID to resume (optional — if omitted, shows a list)
        #[arg(value_name = "SESSION_ID")]
        session_id: Option<String>,
    },
}
```

### `src/error.rs` — Modified

**New error variants:**

```rust
#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // ... existing variants ...

    // Session errors (Phase 8a)
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Session error: {0}")]
    SessionError(String),
}
```

### `src/main.rs` — Modified

**Updated command dispatch:**

```rust
match &cli.command {
    Some(Commands::Ask { question }) => {
        run_oneshot(&config, question).await?;
    }
    Some(Commands::Resume { session_id }) => {
        run_resume(&config, session_id.as_deref()).await?;
    }
    None => {
        run_repl(&config).await?;
    }
}
```

---

## Sub-Phase 8a.6: Slash Commands

### `src/repl.rs` — Modified

**New `run_resume()` entry point:**

```rust
/// Resume a previous session.
///
/// If `session_id` is provided, resumes that specific session.
/// Otherwise, lists recent sessions for the user to pick.
pub async fn run_resume(
    config: &Config,
    session_id_str: Option<&str>,
) -> anyhow::Result<()> {
    let store = SessionStore::new(config.sessions_dir.clone());

    let session_id = if let Some(id_str) = session_id_str {
        SessionId::parse(id_str).map_err(|e| {
            anyhow::anyhow!("Invalid session ID '{}': {}", id_str, e)
        })?
    } else {
        // List recent sessions and let user pick
        let sessions = store.list_sessions()?;
        if sessions.is_empty() {
            println!("No sessions found.");
            return Ok(());
        }

        println!("Recent sessions:\n");
        let max_display = 10.min(sessions.len());
        for (i, meta) in sessions.iter().take(max_display).enumerate() {
            let preview = meta.preview.as_deref().unwrap_or("(no messages)");
            println!(
                "  [{}] {} — \"{}\" ({}, {} events)",
                i + 1,
                meta.relative_time(),
                preview,
                meta.mode,
                meta.event_count,
            );
        }
        println!();

        // Read user selection
        print!("Select session (1-{}): ", max_display);
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let choice: usize = input.trim().parse().unwrap_or(0);

        if choice < 1 || choice > max_display {
            println!("Invalid selection.");
            return Ok(());
        }

        sessions[choice - 1].session_id.clone()
    };

    // Load events and reconstruct history
    let events = store.load_events(&session_id)?;
    let history = SessionStore::reconstruct_history(&events);

    println!(
        "Resumed session {} ({} turns restored)\n",
        &session_id.as_str()[..8],
        history.len(),
    );

    // Create orchestrator and restore history
    // ... (same as run_repl but with set_history + set_session)
    let client = Arc::new(GeminiClient::new(&config.api_key, &config.model));
    let sandbox = crate::sandbox::create_sandbox(
        config.sandbox_mode,
        config.working_directory.clone(),
    );

    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
        Arc::new(DiffOnlyApprovalHandler::new()),
        config.personality,
        config.context_window_turns,
        sandbox,
        config.protected_paths.clone(),
    );

    orchestrator.set_history(history);
    orchestrator.set_session(session_id, store);

    // Continue with normal REPL loop
    // ... (reuse existing REPL logic)
    Ok(())
}
```

**New slash commands in `handle_slash_command()`:**

```rust
"/resume" => {
    let store = orchestrator.session_store().cloned();
    if let Some(store) = store {
        let sessions = store.list_sessions().unwrap_or_default();
        if sessions.is_empty() {
            println!("No sessions found.");
        } else {
            let max_display = 10.min(sessions.len());
            println!("Recent sessions:\n");
            for (i, meta) in sessions.iter().take(max_display).enumerate() {
                let preview = meta.preview.as_deref().unwrap_or("(no messages)");
                println!(
                    "  [{}] {} — \"{}\" ({}, {} events)",
                    i + 1,
                    meta.relative_time(),
                    preview,
                    meta.mode,
                    meta.event_count,
                );
            }
            println!("\nUse `closed-code resume <session-id>` to resume a session.");
        }
    } else {
        println!("Session persistence is not enabled.");
    }
    SlashResult::Continue
}

"/new" => {
    // End current session, start fresh
    orchestrator.clear_history();
    println!("Started new session.");
    if let Some(id) = orchestrator.session_id() {
        println!("Session ID: {}", &id.as_str()[..8]);
    }
    SlashResult::Continue
}

"/fork" => {
    match orchestrator.fork_session() {
        Ok(Some(new_id)) => {
            println!(
                "Forked → new session {}. Original preserved.",
                &new_id.as_str()[..8],
            );
        }
        Ok(None) => {
            println!("Session persistence is not enabled.");
        }
        Err(e) => {
            eprintln!("Fork failed: {}", e);
        }
    }
    SlashResult::Continue
}

"/compact" => {
    let keep_recent = if arg.is_empty() {
        5
    } else {
        arg.parse::<usize>().unwrap_or(5)
    };

    let turns_before = orchestrator.turn_count();
    println!("Compacting {} turns (keeping last {})...", turns_before, keep_recent);

    match orchestrator.compact_history(keep_recent).await {
        Ok(_summary) => {
            let turns_after = orchestrator.turn_count();
            println!(
                "Compacted: {} turns → {} turns",
                turns_before,
                turns_after,
            );
        }
        Err(e) => {
            eprintln!("Compact failed: {}", e);
        }
    }
    SlashResult::Continue
}

"/history" => {
    let count = if arg.is_empty() {
        10
    } else {
        arg.parse::<usize>().unwrap_or(10)
    };

    let history = orchestrator.history();
    let start = history.len().saturating_sub(count);

    for (i, content) in history[start..].iter().enumerate() {
        let role = content.role.as_deref().unwrap_or("system");
        let text: String = content.parts.iter().filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("");

        if !text.is_empty() {
            let truncated = if text.len() > 120 {
                format!("{}...", &text[..117])
            } else {
                text
            };
            println!("  [{}] {}: {}", start + i + 1, role, truncated);
        }
    }
    SlashResult::Continue
}

"/export" => {
    let filename = if arg.is_empty() {
        "transcript.md"
    } else {
        arg
    };

    match orchestrator.session_store() {
        Some(store) => {
            if let Some(session_id) = orchestrator.session_id() {
                match store.load_events(session_id) {
                    Ok(events) => {
                        let writer = crate::session::transcript::TranscriptWriter;
                        match writer.write_to_file(&events, filename) {
                            Ok(()) => println!("Exported to {}", filename),
                            Err(e) => eprintln!("Export failed: {}", e),
                        }
                    }
                    Err(e) => eprintln!("Failed to load session: {}", e),
                }
            }
        }
        None => println!("Session persistence is not enabled."),
    }
    SlashResult::Continue
}
```

**Updated `/help`:**

```
/resume           — List and resume previous sessions
/new              — Start a new session (archives current)
/fork             — Fork current session into a new branch
/compact [N]      — Summarize conversation, keep last N turns (default 5)
/history [N]      — Show last N conversation turns (default 10)
/export [file]    — Export session to markdown (default: transcript.md)
```

**Updated `/status`:**

```
Mode: explore | Model: gemini-3.1-pro-preview | Personality: pragmatic
Session: a1b2c3d4 (auto-save enabled)
Sandbox: workspace-write (macOS Seatbelt)
Git: main (clean)
Tokens: 1,234 prompt + 567 completion = 1,801 total (3 API calls)
Turns: 4 / 50 | Tools: 9
```

**Updated REPL startup to auto-create session:**

```rust
pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    // ... existing client + sandbox + orchestrator setup ...

    // Session setup
    if config.session_auto_save {
        let store = SessionStore::new(config.sessions_dir.clone());
        orchestrator.start_session(store);
    }

    // ... startup banner ...
    if let Some(id) = orchestrator.session_id() {
        println!("Session: {}", &id.as_str()[..8]);
    }

    // ... existing REPL loop ...

    // Emit session end on exit
    if orchestrator.session_id().is_some() {
        orchestrator.emit_event(SessionEvent::SessionEnd {
            timestamp: Utc::now(),
        });
    }

    Ok(())
}
```

---

## Sub-Phase 8a.7: TranscriptWriter

### New File: `src/session/transcript.rs`

Optional markdown export of session events.

```rust
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::error::{ClosedCodeError, Result};
use super::SessionEvent;

/// Writes session events to a human-readable markdown file.
pub struct TranscriptWriter;

impl TranscriptWriter {
    /// Export session events to a markdown file.
    pub fn write_to_file(&self, events: &[SessionEvent], path: &str) -> Result<()> {
        let markdown = self.render(events);

        let mut file = fs::File::create(path).map_err(|e| {
            ClosedCodeError::SessionError(format!("Failed to create {}: {}", path, e))
        })?;

        file.write_all(markdown.as_bytes()).map_err(|e| {
            ClosedCodeError::SessionError(format!("Failed to write {}: {}", path, e))
        })?;

        Ok(())
    }

    /// Render session events to a markdown string.
    pub fn render(&self, events: &[SessionEvent]) -> String {
        let mut md = String::new();

        md.push_str("# Session Transcript\n\n");

        // Extract session info from first event
        if let Some(SessionEvent::SessionStart {
            session_id,
            model,
            mode,
            working_directory,
            timestamp,
        }) = events.first()
        {
            md.push_str(&format!("- **Session**: {}\n", session_id));
            md.push_str(&format!("- **Model**: {}\n", model));
            md.push_str(&format!("- **Mode**: {}\n", mode));
            md.push_str(&format!("- **Directory**: {}\n", working_directory));
            md.push_str(&format!(
                "- **Started**: {}\n",
                timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            ));
            md.push_str("\n---\n\n");
        }

        for event in events {
            match event {
                SessionEvent::UserMessage { content, timestamp } => {
                    md.push_str(&format!(
                        "### User ({})\n\n{}\n\n",
                        timestamp.format("%H:%M:%S"),
                        content,
                    ));
                }
                SessionEvent::AssistantMessage { content, timestamp } => {
                    md.push_str(&format!(
                        "### Assistant ({})\n\n{}\n\n",
                        timestamp.format("%H:%M:%S"),
                        content,
                    ));
                }
                SessionEvent::ToolCall { name, args, timestamp } => {
                    md.push_str(&format!(
                        "> **Tool call** ({}): `{}`\n>\n> ```json\n> {}\n> ```\n\n",
                        timestamp.format("%H:%M:%S"),
                        name,
                        serde_json::to_string_pretty(args).unwrap_or_default(),
                    ));
                }
                SessionEvent::ToolResponse { name, result, timestamp } => {
                    let result_str = serde_json::to_string_pretty(result)
                        .unwrap_or_default();
                    let truncated = if result_str.len() > 500 {
                        format!("{}...\n(truncated)", &result_str[..500])
                    } else {
                        result_str
                    };
                    md.push_str(&format!(
                        "> **Tool result** ({}): `{}`\n>\n> ```json\n> {}\n> ```\n\n",
                        timestamp.format("%H:%M:%S"),
                        name,
                        truncated,
                    ));
                }
                SessionEvent::ModeChange { from, to, timestamp } => {
                    md.push_str(&format!(
                        "*Mode changed from {} to {} at {}*\n\n",
                        from,
                        to,
                        timestamp.format("%H:%M:%S"),
                    ));
                }
                SessionEvent::Compact { summary, turns_before, turns_after, timestamp } => {
                    md.push_str(&format!(
                        "---\n\n*Compacted at {}: {} turns → {} turns*\n\n**Summary**: {}\n\n---\n\n",
                        timestamp.format("%H:%M:%S"),
                        turns_before,
                        turns_after,
                        summary,
                    ));
                }
                SessionEvent::ImageAttached { source, description, timestamp } => {
                    md.push_str(&format!(
                        "> **Image** ({}, {})\n>\n> {}\n\n",
                        source,
                        timestamp.format("%H:%M:%S"),
                        description,
                    ));
                }
                SessionEvent::SessionStart { .. } | SessionEvent::SessionEnd { .. } => {
                    // Handled separately or ignored
                }
            }
        }

        // End marker
        if let Some(SessionEvent::SessionEnd { timestamp }) = events.last() {
            md.push_str(&format!(
                "---\n\n*Session ended at {}*\n",
                timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            ));
        }

        md
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| Tool results truncated at 500 chars | Tool outputs can be enormous (file contents, directory listings). The transcript stays readable. |
| Markdown format | Human-readable, renderable in any markdown viewer, good for archival. |
| Timestamps in HH:MM:SS for events | Concise. Full date is in the header. |
| `TranscriptWriter` is a zero-size struct | No state needed. Pure function wrapped in a type for consistency and future extensibility. |

---

## Test Summary

| File | New Tests | Category |
|------|-----------|----------|
| `src/session/mod.rs` | 14 | SessionId::new (uniqueness), SessionId::parse (valid, invalid), SessionId::display, SessionEvent serde roundtrip for all 9 variants, SessionEvent::timestamp accessor, SessionMeta::relative_time (minutes, hours, days), SessionMeta preview truncation |
| `src/session/store.rs` | 28 | SessionStore::new, default_dir, ensure_dir, save_event (creates file, appends), load_events (valid, empty, malformed line skipped), list_sessions (empty dir, multiple sessions, sorted by recent), read_session_meta (valid, invalid first line), delete_session (exists, not found), fork_session (success, source not found), reconstruct_history (basic, with compact, tool calls, image attached, empty), session_path format |
| `src/session/transcript.rs` | 8 | TranscriptWriter::render (basic conversation, with tools, with compact, with image, empty events), write_to_file (success, bad path), markdown format correctness |
| `src/agent/orchestrator.rs` | 10 | set_session/session_id, emit_event with no store (no-op), emit_event with store, set_history, fork_session, clear_history emits events, start_session creates ID, history accessor |
| `src/config.rs` | 6 | SessionConfig TOML parsing (full, empty, defaults), Config::from_cli with session settings, merge session config, default session_auto_save true |
| `src/cli.rs` | 4 | Parse resume subcommand (with ID, without ID), resume defaults to None, existing commands unchanged |
| `src/error.rs` | 4 | SessionNotFound display, SessionError display, both contain message text |
| `src/repl.rs` | 12 | /resume returns Continue, /new clears history, /fork returns Continue, /compact returns Continue, /history shows turns, /export returns Continue, /help includes new commands, /status includes session ID, session auto-start on REPL launch, session end on REPL exit, run_resume with invalid ID, run_resume with valid ID |
| **Total** | **86 new tests** | |

---

## Milestone

```bash
# Auto-save session on REPL startup
cargo run -- --api-key $KEY
# closed-code
# Mode: explore | Model: gemini-3.1-pro-preview | Tools: 9
# Working directory: /Users/me/project
# Sandbox: workspace-write (macOS Seatbelt)
# Session: a1b2c3d4
# Git: main (clean)
# Type /help for commands, Ctrl+C to interrupt, /quit to exit.

# Normal conversation (events auto-saved to ~/.closed-code/sessions/a1b2c3d4-....jsonl)
# explore > What files are in this project?
# ⠋ Using list_directory...
# There are 5 files: Cargo.toml, src/main.rs, ...

# explore > /status
# Mode: explore | Model: gemini-3.1-pro-preview | Personality: pragmatic
# Session: a1b2c3d4 (auto-save enabled)
# Sandbox: workspace-write (macOS Seatbelt)
# Git: main (clean)
# Tokens: 1,234 prompt + 567 completion = 1,801 total (3 API calls)
# Turns: 4 / 50 | Tools: 9

# explore > /quit

# Resume the session
cargo run -- resume
# Recent sessions:
#
#   [1] 2m ago — "What files are in this project?" (explore, 5 events)
#   [2] 1d ago — "Explain the auth flow" (plan, 23 events)
#   [3] 3d ago — "Add caching to API" (execute, 47 events)
#
# Select session (1-3): 1
# Resumed session a1b2c3d4 (4 turns restored)
#
# explore > Tell me more about the main.rs file
# (continues with full context from previous session)

# Resume by ID
cargo run -- resume a1b2c3d4-5678-...
# Resumed session a1b2c3d4 (4 turns restored)

# Fork
# explore > /fork
# Forked → new session b2c3d4e5. Original preserved.

# Compact
# explore > /compact
# Compacting 47 turns (keeping last 5)...
# Compacted: 47 turns → 6 turns

# explore > /compact 10
# Compacting 23 turns (keeping last 10)...
# Compacted: 23 turns → 11 turns

# History
# explore > /history
#   [38] user: Can you also add error handling?
#   [39] model: Sure! I'll add proper error handling with...
#   [40] user: /accept
#   ...
#   [47] model: All changes have been applied.

# Export
# explore > /export review.md
# Exported to review.md

# /new
# explore > /new
# Started new session.
# Session: c3d4e5f6

# Config
# ~/.closed-code/config.toml:
# [session]
# auto_save = true
# transcript_logging = false

# Disable auto-save
# [session]
# auto_save = false

# Tests
cargo test
# running 471 tests (385 existing + 86 new)
# test session::tests::... ok
# test session::store::tests::... ok
# test session::transcript::tests::... ok
# ...
# test result: ok. 471 passed; 0 failed
```

---

## Implementation Order

1. `Cargo.toml` — add `chrono = { version = "0.4", features = ["serde"] }`
2. `src/error.rs` — add `SessionNotFound(String)`, `SessionError(String)` variants
3. `src/session/mod.rs` — `SessionId`, `SessionEvent` enum (all 9 variants), `SessionMeta`
4. `src/lib.rs` — add `pub mod session;`
5. `cargo test` checkpoint — session type serde tests pass
6. `src/session/store.rs` — `SessionStore` with save_event, load_events, list_sessions, delete_session, fork_session, reconstruct_history
7. `cargo test` checkpoint — store persistence tests pass (with temp dirs)
8. `src/session/transcript.rs` — `TranscriptWriter` with render and write_to_file
9. `cargo test` checkpoint — transcript rendering tests pass
10. `src/agent/orchestrator.rs` — add session fields, emit_event, set_session, set_history, fork_session, compact_history, start_session
11. `cargo test` checkpoint — orchestrator session integration tests pass
12. `src/config.rs` — add `SessionConfig`, session fields to `Config`, merge logic
13. `src/cli.rs` — add `Resume` subcommand
14. `cargo test` checkpoint — config + CLI parsing tests pass
15. `src/repl.rs` — add 6 slash commands, session auto-start/end, run_resume
16. `src/main.rs` — add `Resume` command dispatch
17. `cargo test` — all 471 tests pass (385 existing + 86 new)

---

## Complexity: **Medium-High**

JSONL persistence is straightforward, but the orchestrator integration requires careful threading of events through the existing `handle_user_input_streaming()` code path. The compact operation involves a dedicated API call for summarization. Fork and resume require history reconstruction from serialized events. ~3 new files, ~8 modified files, ~86 new tests, ~1,800 lines.

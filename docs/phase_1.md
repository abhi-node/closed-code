# Phase 1: Foundation + Gemini Client + Basic Conversation

**Goal**: A working binary that holds a multi-turn streaming conversation with Gemini via the terminal.

**Deliverable**: `cargo run` launches a REPL where you can chat with Gemini 3.1 Pro Preview. Streaming token display, conversation history, basic slash commands, one-shot `ask` subcommand.

**Estimated**: ~3,000 lines of Rust across ~15 files.

---

## File Layout

```
closed-code/
  Cargo.toml                 # Package manifest, all Phase 1 dependencies
  src/
    main.rs                  # Entry point: tokio runtime, CLI dispatch, panic hook
    cli.rs                   # Clap derive: --mode, --directory, --api-key, --model, --verbose, ask subcommand
    config.rs                # Config struct assembled from env vars + CLI flags (no TOML yet)
    error.rs                 # ClosedCodeError enum via thiserror, is_retryable() helper
    lib.rs                   # Crate root: module declarations and re-exports
    mode/
      mod.rs                 # Mode enum (Explore, Plan, Execute) with Display/FromStr
    gemini/
      mod.rs                 # Module re-exports
      types.rs               # Full Gemini API serde types + custom Part deserializer
      client.rs              # GeminiClient: generate_content(), stream_generate_content(), retry logic
      stream.rs              # SSE parser: reqwest-eventsource → typed stream of StreamEvent
    ui/
      mod.rs                 # Module re-exports
      theme.rs               # ANSI color constants (user, assistant, error, success, dim, accent)
      spinner.rs             # indicatif spinner wrapper with configurable messages
    repl.rs                  # REPL: rustyline input, conversation history, streaming display, slash commands
```

---

## Dependencies

```toml
[package]
name = "closed-code"
version = "0.1.0"
edition = "2021"
description = "AI-powered coding CLI powered by Gemini"

[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"

# CLI argument parsing
clap = { version = "4", features = ["derive", "env"] }

# HTTP + SSE streaming
reqwest = { version = "0.12", features = ["json", "stream"] }
reqwest-eventsource = "0.6"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Terminal I/O and UI
crossterm = "0.28"
indicatif = "0.17"
rustyline = { version = "15", features = ["derive"] }

# Error handling
thiserror = "2"
anyhow = "1"

# Retry with backoff
backon = "1"

# Async traits
async-trait = "0.1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Utilities
uuid = { version = "1", features = ["v4", "serde"] }
dirs = "6"
futures = "0.3"
```

### Crate Rationale

| Crate | Why |
|-------|-----|
| `tokio` | Async runtime — full features for fs, process, signal, sync |
| `reqwest` | HTTP client with JSON body and streaming response support |
| `reqwest-eventsource` | SSE stream consumer built on reqwest; handles `data:` line parsing |
| `serde` / `serde_json` | Gemini API JSON serialization/deserialization |
| `clap` | CLI argument parsing with derive macros and env var support |
| `crossterm` | Terminal manipulation — colors, cursor, raw mode (used by rustyline internally) |
| `indicatif` | Animated spinner for "Thinking..." states |
| `rustyline` | Line editing with history, completion, Ctrl+C/Ctrl+D handling |
| `thiserror` | Derive macro for ergonomic error enum definition |
| `anyhow` | Convenient error propagation in main/top-level functions |
| `backon` | Ergonomic exponential backoff retry — actively maintained, native async |
| `async-trait` | `async fn` in trait definitions (until RPITIT stabilizes fully) |
| `tracing` | Structured logging with `RUST_LOG` env filter |
| `uuid` | Session IDs (v4 random) — needed from Phase 1 for future session support |
| `dirs` | Platform-specific directories (`~/.closed-code/`) |
| `futures` | `StreamExt` for consuming async streams |

---

## Gemini API Reference

### Authentication

Use the `x-goog-api-key` HTTP header (preferred over `?key=` query parameter for security — keys don't leak into server logs/URLs):

```
x-goog-api-key: {GEMINI_API_KEY}
```

### Endpoints

**Base URL**: `https://generativelanguage.googleapis.com/v1beta`

| Operation | Method | URL |
|-----------|--------|-----|
| Generate (non-streaming) | POST | `/models/{model}:generateContent` |
| Generate (SSE streaming) | POST | `/models/{model}:streamGenerateContent?alt=sse` |

Model ID: `gemini-3.1-pro-preview` (as specified by user).

### Request Schema

```json
{
  "contents": [
    {
      "role": "user" | "model",
      "parts": [
        { "text": "string" },
        { "functionCall": { "name": "string", "args": { ... } } },
        { "functionResponse": { "name": "string", "response": { ... } } },
        { "inlineData": { "mimeType": "string", "data": "base64" } }
      ]
    }
  ],
  "systemInstruction": {
    "parts": [{ "text": "system prompt text" }]
  },
  "tools": [
    {
      "functionDeclarations": [
        {
          "name": "function_name",
          "description": "What the function does",
          "parameters": {
            "type": "object",
            "properties": { ... },
            "required": ["param1"]
          }
        }
      ]
    }
  ],
  "toolConfig": {
    "functionCallingConfig": {
      "mode": "AUTO" | "ANY" | "NONE"
    }
  },
  "generationConfig": {
    "temperature": 1.0,
    "topP": 0.95,
    "topK": 40,
    "maxOutputTokens": 8192,
    "stopSequences": []
  }
}
```

**Notes**:
- All JSON field names are **camelCase** (`functionCall`, `systemInstruction`, `toolConfig`, `functionCallingConfig`).
- `contents` is the conversation history. `role` alternates between `"user"` and `"model"`.
- `functionResponse` parts are sent with `role: "user"` (the user provides tool results back).
- `tools` and `toolConfig` are omitted in Phase 1 (no function calling yet — that's Phase 2).
- `systemInstruction` has no `role` field — just `parts`.

### Response Schema

```json
{
  "candidates": [
    {
      "content": {
        "role": "model",
        "parts": [
          { "text": "response text" },
          { "functionCall": { "name": "fn", "args": { ... } } }
        ]
      },
      "finishReason": "STOP" | "MAX_TOKENS" | "SAFETY" | "RECITATION" | "OTHER",
      "safetyRatings": [
        {
          "category": "HARM_CATEGORY_...",
          "probability": "NEGLIGIBLE" | "LOW" | "MEDIUM" | "HIGH"
        }
      ]
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 100,
    "candidatesTokenCount": 50,
    "totalTokenCount": 150
  },
  "modelVersion": "gemini-3.1-pro-preview"
}
```

### SSE Streaming Format

The `streamGenerateContent?alt=sse` endpoint returns `Content-Type: text/event-stream`. Each event is a complete `GenerateContentResponse` JSON:

```
data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}\n\n
data: {"candidates":[{"content":{"parts":[{"text":" world"}]}}]}\n\n
data: {"candidates":[{"content":{"parts":[{"text":"!"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3,"totalTokenCount":8}}\n\n
```

Each `data:` line contains one JSON object. The final chunk includes `finishReason` and `usageMetadata`. Text is delivered incrementally — each chunk contains a fragment, not the full text.

---

## Implementation Details

### `src/main.rs`

Entry point. Sets up tokio runtime, parses CLI, initializes config, dispatches to REPL or one-shot mode.

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Install panic hook to restore terminal on panic
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        default_hook(info);
    }));

    // 2. Initialize tracing (RUST_LOG env filter)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 3. Parse CLI args
    let cli = Cli::parse();

    // 4. Build config from CLI + env
    let config = Config::from_cli(&cli)?;

    // 5. Dispatch
    match &cli.command {
        Some(Commands::Ask { question }) => {
            // One-shot: send question, stream response, exit
            run_oneshot(&config, question).await?;
        }
        None => {
            // Interactive REPL
            run_repl(&config).await?;
        }
    }

    Ok(())
}
```

### `src/cli.rs`

Clap derive definitions.

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "closed-code", version, about = "AI-powered coding CLI")]
pub struct Cli {
    /// Operating mode: explore, plan, or execute
    #[arg(long, default_value = "explore")]
    pub mode: String,

    /// Working directory (defaults to current dir)
    #[arg(long, short = 'd')]
    pub directory: Option<PathBuf>,

    /// Gemini API key (or set GEMINI_API_KEY env var)
    #[arg(long, env = "GEMINI_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    /// Model name
    #[arg(long, default_value = "gemini-3.1-pro-preview")]
    pub model: String,

    /// Enable verbose/debug output
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Send a one-shot question (non-interactive)
    Ask {
        /// The question to ask
        question: String,
    },
}
```

### `src/config.rs`

Assembles runtime configuration from CLI flags and environment variables. No TOML file support yet (that's Phase 5).

```rust
pub struct Config {
    pub api_key: String,
    pub model: String,
    pub mode: Mode,
    pub working_directory: PathBuf,
    pub verbose: bool,
    pub max_output_tokens: u32,
}

impl Config {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let api_key = cli.api_key.clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .ok_or(ClosedCodeError::MissingApiKey)?;

        let mode = cli.mode.parse::<Mode>()?;

        let working_directory = cli.directory.clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        Ok(Self {
            api_key,
            model: cli.model.clone(),
            mode,
            working_directory,
            verbose: cli.verbose,
            max_output_tokens: 8192,
        })
    }
}
```

### `src/error.rs`

Central error type using `thiserror`. Every error variant has context for debugging.

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // API errors
    #[error("Gemini API error (HTTP {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("Rate limited (429). Retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("Gemini API returned no candidates")]
    EmptyResponse,

    #[error("Response blocked by safety filter: {reason}")]
    SafetyBlocked { reason: String },

    // Config errors
    #[error("Missing API key. Set GEMINI_API_KEY or pass --api-key")]
    MissingApiKey,

    #[error("Invalid mode '{0}'. Expected: explore, plan, or execute")]
    InvalidMode(String),

    // Network / IO
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    // Parse errors
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SSE stream error: {0}")]
    StreamError(String),

    #[error("Failed to parse Gemini Part: {0}")]
    PartParseError(String),
}

impl ClosedCodeError {
    /// Whether this error is transient and the request can be retried.
    pub fn is_retryable(&self) -> bool {
        matches!(self,
            Self::RateLimited { .. } |
            Self::Network(_) |
            Self::ApiError { status, .. } if *status >= 500
        )
    }

    /// Build from an HTTP response status + body.
    pub fn from_status(status: u16, body: String) -> Self {
        match status {
            429 => Self::RateLimited { retry_after_ms: 1000 },
            s if s >= 500 => Self::ApiError { status: s, message: body },
            s => Self::ApiError { status: s, message: body },
        }
    }
}

pub type Result<T> = std::result::Result<T, ClosedCodeError>;
```

### `src/mode/mod.rs`

```rust
use std::fmt;
use std::str::FromStr;
use crate::error::ClosedCodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Explore,
    Plan,
    Execute,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mode::Explore => write!(f, "explore"),
            Mode::Plan => write!(f, "plan"),
            Mode::Execute => write!(f, "execute"),
        }
    }
}

impl FromStr for Mode {
    type Err = ClosedCodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "explore" => Ok(Mode::Explore),
            "plan" => Ok(Mode::Plan),
            "execute" => Ok(Mode::Execute),
            other => Err(ClosedCodeError::InvalidMode(other.to_string())),
        }
    }
}
```

### `src/gemini/types.rs`

All serde types for the Gemini API. The key challenge is the `Part` enum — Gemini returns camelCase JSON where the variant is determined by which key is present (`text`, `functionCall`, `functionResponse`, `inlineData`).

**Why a custom deserializer instead of `#[serde(untagged)]`**:
- `untagged` tries each variant sequentially — poor error messages on failure
- `untagged` has performance overhead (backtracking on parse failure)
- Our custom visitor does a single pass: read keys, determine variant

```rust
use serde::{Deserialize, Serialize, Deserializer};
use serde::de::{self, MapAccess, Visitor};
use serde_json::Value;
use std::fmt;

// ── Request Types ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    // tools and tool_config added in Phase 2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

// ── Part Enum (custom deserialization) ──

#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    FunctionCall {
        name: String,
        args: Value,
    },
    FunctionResponse {
        name: String,
        response: Value,
    },
    InlineData {
        mime_type: String,
        data: String,
    },
}

// Serialize: produce the camelCase JSON Gemini expects
impl Serialize for Part {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        use serde::ser::SerializeMap;
        match self {
            Part::Text(text) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("text", text)?;
                map.end()
            }
            Part::FunctionCall { name, args } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("functionCall", &serde_json::json!({
                    "name": name, "args": args
                }))?;
                map.end()
            }
            Part::FunctionResponse { name, response } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("functionResponse", &serde_json::json!({
                    "name": name, "response": response
                }))?;
                map.end()
            }
            Part::InlineData { mime_type, data } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("inlineData", &serde_json::json!({
                    "mimeType": mime_type, "data": data
                }))?;
                map.end()
            }
        }
    }
}

// Deserialize: inspect which JSON key exists to pick the variant
impl<'de> Deserialize<'de> for Part {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: Deserializer<'de> {
        deserializer.deserialize_map(PartVisitor)
    }
}

struct PartVisitor;

impl<'de> Visitor<'de> for PartVisitor {
    type Value = Part;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a Part object with text, functionCall, functionResponse, or inlineData")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where A: MapAccess<'de> {
        // We only expect one key per Part object
        let key: String = map.next_key()?
            .ok_or_else(|| de::Error::custom("empty Part object"))?;

        match key.as_str() {
            "text" => {
                let text: String = map.next_value()?;
                Ok(Part::Text(text))
            }
            "functionCall" => {
                let call: FunctionCallRaw = map.next_value()?;
                Ok(Part::FunctionCall { name: call.name, args: call.args })
            }
            "functionResponse" => {
                let resp: FunctionResponseRaw = map.next_value()?;
                Ok(Part::FunctionResponse { name: resp.name, response: resp.response })
            }
            "inlineData" => {
                let data: InlineDataRaw = map.next_value()?;
                Ok(Part::InlineData { mime_type: data.mime_type, data: data.data })
            }
            other => Err(de::Error::unknown_field(other,
                &["text", "functionCall", "functionResponse", "inlineData"])),
        }
    }
}

#[derive(Deserialize)]
struct FunctionCallRaw {
    name: String,
    args: Value,
}

#[derive(Deserialize)]
struct FunctionResponseRaw {
    name: String,
    response: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineDataRaw {
    mime_type: String,
    data: String,
}

// ── Response Types ──

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentResponse {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    pub usage_metadata: Option<UsageMetadata>,
    pub model_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub content: Option<Content>,
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub safety_ratings: Vec<SafetyRating>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    pub prompt_token_count: Option<u32>,
    pub candidates_token_count: Option<u32>,
    pub total_token_count: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SafetyRating {
    pub category: String,
    pub probability: String,
}

// ── Helper constructors ──

impl Content {
    pub fn user(text: &str) -> Self {
        Content {
            role: Some("user".into()),
            parts: vec![Part::Text(text.into())],
        }
    }

    pub fn model(text: &str) -> Self {
        Content {
            role: Some("model".into()),
            parts: vec![Part::Text(text.into())],
        }
    }

    pub fn system(text: &str) -> Self {
        Content {
            role: None,
            parts: vec![Part::Text(text.into())],
        }
    }
}

impl GenerateContentResponse {
    /// Extract the text from the first candidate's first text part.
    pub fn text(&self) -> Option<&str> {
        self.candidates.first()
            .and_then(|c| c.content.as_ref())
            .and_then(|content| content.parts.first())
            .and_then(|part| match part {
                Part::Text(t) => Some(t.as_str()),
                _ => None,
            })
    }
}
```

### `src/gemini/client.rs`

HTTP client with retry logic. Uses `backon` for exponential backoff on transient errors (429, 5xx).

```rust
pub struct GeminiClient {
    client: reqwest::Client,
    api_key: String,   // never logged
    model: String,
    base_url: String,
}

impl GeminiClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
        }
    }

    fn url(&self, method: &str) -> String {
        format!("{}/models/{}:{}", self.base_url, self.model, method)
    }

    /// Non-streaming generate (used by sub-agents in later phases).
    pub async fn generate_content(
        &self,
        request: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse> {
        // Retry with backon: 500ms → 1s → 2s, max 3 attempts
        let response = (|| async {
            let resp = self.client
                .post(self.url("generateContent"))
                .header("x-goog-api-key", &self.api_key)
                .json(request)
                .send()
                .await?;
            Ok::<_, reqwest::Error>(resp)
        })
        .retry(backon::ExponentialBuilder::default()
            .with_min_delay(Duration::from_millis(500))
            .with_max_times(3))
        .sleep(tokio::time::sleep)
        .when(|e: &reqwest::Error| {
            e.is_timeout() || e.is_connect() || e.status()
                .map(|s| s == 429 || s.is_server_error())
                .unwrap_or(false)
        })
        .notify(|err, dur| {
            tracing::warn!("Retrying after {:?}: {}", dur, err);
        })
        .await
        .map_err(ClosedCodeError::Network)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ClosedCodeError::from_status(status.as_u16(), body));
        }

        let result: GenerateContentResponse = response.json().await?;
        Ok(result)
    }

    /// Streaming generate — returns an SSE event source.
    /// Caller consumes events via `stream.rs` helpers.
    pub async fn stream_generate_content(
        &self,
        request: &GenerateContentRequest,
    ) -> reqwest_eventsource::EventSource {
        let request_builder = self.client
            .post(format!("{}?alt=sse", self.url("streamGenerateContent")))
            .header("x-goog-api-key", &self.api_key)
            .json(request);

        reqwest_eventsource::EventSource::new(request_builder)
            .expect("failed to create EventSource")
    }
}

// Manual Debug impl to redact API key
impl fmt::Debug for GeminiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiClient")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}
```

### `src/gemini/stream.rs`

Consumes the SSE `EventSource` and yields typed events. Handles both text streaming and function call detection (for Phase 2+).

```rust
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};

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
                let response: GenerateContentResponse =
                    serde_json::from_str(&msg.data)
                        .map_err(|e| ClosedCodeError::StreamError(
                            format!("Failed to parse SSE data: {e}")
                        ))?;

                if let Some(candidate) = response.candidates.first() {
                    if let Some(content) = &candidate.content {
                        for part in &content.parts {
                            match part {
                                Part::Text(text) => {
                                    full_text.push_str(text);
                                    on_event(StreamEvent::TextDelta(text.clone()));
                                }
                                Part::FunctionCall { .. } => {
                                    // Buffer function calls (Phase 2+)
                                    on_event(StreamEvent::FunctionCall(response.clone()));
                                    es.close();
                                    return Ok(full_text);
                                }
                                _ => {}
                            }
                        }
                    }

                    // Check for finish
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
```

### `src/ui/theme.rs`

ANSI color constants via crossterm. Centralized so the entire app has consistent styling.

```rust
use crossterm::style::Color;

pub struct Theme;

impl Theme {
    pub const USER: Color = Color::Cyan;
    pub const ASSISTANT: Color = Color::White;
    pub const ERROR: Color = Color::Red;
    pub const SUCCESS: Color = Color::Green;
    pub const DIM: Color = Color::DarkGrey;
    pub const ACCENT: Color = Color::Yellow;
    pub const PROMPT: Color = Color::Blue;
}
```

### `src/ui/spinner.rs`

Wrapper around `indicatif` for a "Thinking..." spinner.

```rust
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    pub fn new(message: &str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
                .template("{spinner} {msg}")
                .unwrap(),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        bar.set_message(message.to_string());
        Self { bar }
    }

    pub fn set_message(&self, message: &str) {
        self.bar.set_message(message.to_string());
    }

    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }
}
```

### `src/repl.rs`

The interactive REPL. Uses `rustyline` for line input (history, line editing, Ctrl+C/Ctrl+D). Streams responses via the Gemini client.

**Architecture**: rustyline is synchronous but runs inside `tokio::task::spawn_blocking` wouldn't work cleanly. Instead, the REPL loop is `async` — rustyline's `readline()` is fast (it blocks on user input, which is fine in the main loop since we're not doing anything else while waiting for input).

```rust
use rustyline::{DefaultEditor, Result as RlResult};
use rustyline::error::ReadlineError;

pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    let client = GeminiClient::new(config.api_key.clone(), config.model.clone());
    let mut history: Vec<Content> = Vec::new();
    let mut editor = DefaultEditor::new()?;

    let system_prompt = build_system_prompt(config);

    // Print welcome
    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!("Mode: {} | Model: {}", config.mode, config.model);
    println!("Type /help for commands, /quit to exit.\n");

    loop {
        let prompt = format!("{} > ", config.mode);
        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() { continue; }
                editor.add_history_entry(line)?;

                // Slash commands
                if line.starts_with('/') {
                    match handle_slash_command(line, &mut history) {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                    }
                    continue;
                }

                // Build request
                history.push(Content::user(line));

                let request = GenerateContentRequest {
                    contents: history.clone(),
                    system_instruction: Some(Content::system(&system_prompt)),
                    generation_config: Some(GenerationConfig {
                        temperature: Some(1.0),
                        top_p: None,
                        top_k: None,
                        max_output_tokens: Some(config.max_output_tokens),
                    }),
                };

                // Show spinner, then stream
                let spinner = Spinner::new("Thinking...");

                let es = client.stream_generate_content(&request).await;
                spinner.finish();

                let full_text = consume_stream(es, |event| {
                    match event {
                        StreamEvent::TextDelta(text) => {
                            print!("{}", text);
                            // Flush to show tokens immediately
                            use std::io::Write;
                            std::io::stdout().flush().ok();
                        }
                        StreamEvent::Done { usage, .. } => {
                            println!(); // newline after streamed response
                            if let Some(u) = usage {
                                tracing::debug!(
                                    "Tokens: {} prompt + {} completion = {} total",
                                    u.prompt_token_count.unwrap_or(0),
                                    u.candidates_token_count.unwrap_or(0),
                                    u.total_token_count.unwrap_or(0),
                                );
                            }
                        }
                        _ => {}
                    }
                }).await;

                match full_text {
                    Ok(text) => {
                        history.push(Content::model(&text));
                    }
                    Err(e) => {
                        eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
                    }
                }
                println!(); // spacing between turns
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C — cancel current input, continue loop
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D — exit
                println!("Goodbye!");
                break;
            }
            Err(e) => {
                eprintln!("Input error: {}", e);
                break;
            }
        }
    }

    Ok(())
}

enum SlashResult {
    Continue,
    Quit,
}

fn handle_slash_command(input: &str, history: &mut Vec<Content>) -> SlashResult {
    match input {
        "/quit" | "/exit" | "/q" => SlashResult::Quit,
        "/clear" => {
            history.clear();
            println!("Conversation history cleared.");
            SlashResult::Continue
        }
        "/help" => {
            println!("Commands:");
            println!("  /help   — Show this help");
            println!("  /clear  — Clear conversation history");
            println!("  /quit   — Exit");
            SlashResult::Continue
        }
        _ => {
            println!("Unknown command: {}. Type /help for available commands.", input);
            SlashResult::Continue
        }
    }
}
```

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **rustyline** over raw crossterm input | Rustyline provides line editing, history, and Ctrl+C/D handling out of the box. Raw crossterm would require reimplementing all of this. We switch to full TUI with crossterm in Phase 9. |
| **Custom Part deserializer** over `#[serde(untagged)]` | Single-pass parsing with clear error messages. `untagged` tries each variant sequentially with backtracking — slower and produces confusing errors like "data did not match any variant". |
| **`backon`** over manual retry loops | Actively maintained crate (v1.0+) with ergonomic fluent API, native tokio async support, and configurable jitter. Manual loops are error-prone and verbose. |
| **`x-goog-api-key` header** over `?key=` query param | API keys in URLs leak into server access logs, browser history, and proxy logs. Headers are safer. |
| **`serde(rename_all = "camelCase")`** on request/response structs | Gemini API uses camelCase for all JSON field names. This attribute handles the mapping automatically for standard fields. |
| **`reqwest-eventsource`** over manual SSE parsing | Battle-tested SSE consumer with built-in reconnection logic. Parsing SSE manually (splitting on `data:` lines) is fragile. |
| **`anyhow`** at top-level, **`thiserror`** for library code | `anyhow::Result` for `main()` and REPL (ergonomic error propagation). `ClosedCodeError` via `thiserror` for typed errors in the library core (pattern matching, `is_retryable()`). |
| **No `async-stream` / channel for REPL** | In Phase 1, the REPL is simple enough to not need channels. `rustyline::readline()` blocks until user presses Enter, then we make the async API call. Channels become necessary in Phase 9 (TUI) when input and output are concurrent. |

---

## Milestone / Verification

After implementing Phase 1, verify each capability:

```bash
# 1. CLI help works
cargo run -- --help
# Expected: shows --mode, --directory, --api-key, --model, --verbose flags
#           and "ask" subcommand

# 2. Missing API key gives clear error
cargo run
# Expected: "Error: Missing API key. Set GEMINI_API_KEY or pass --api-key"

# 3. One-shot streaming query
export GEMINI_API_KEY="your-key"
cargo run -- ask "What is Rust?"
# Expected: streams response token-by-token, then exits

# 4. Interactive REPL launches
cargo run
# Expected:
#   closed-code
#   Mode: explore | Model: gemini-3.1-pro-preview
#   Type /help for commands, /quit to exit.
#
#   explore >

# 5. Multi-turn conversation works
# explore > Hello, who are you?
# (streaming response)
# explore > Tell me more about that
# (streaming response referencing previous context)

# 6. Slash commands work
# explore > /help
# Commands:
#   /help   — Show this help
#   /clear  — Clear conversation history
#   /quit   — Exit
#
# explore > /clear
# Conversation history cleared.
#
# explore > /quit
# Goodbye!

# 7. Ctrl+C cancels input (not exit)
# explore > (type something, press Ctrl+C)
# ^C
# explore >   ← still in REPL

# 8. Ctrl+D exits
# explore > (press Ctrl+D)
# Goodbye!

# 9. Verbose mode shows token usage
RUST_LOG=debug cargo run -- ask "Hello"
# Expected: debug output includes "Tokens: X prompt + Y completion = Z total"

# 10. Retry on transient error (manual test)
# Set an invalid API key, observe retry attempts in debug logs
# Expected: "Retrying after 500ms: ..." up to 3 times, then error
```

---

## What This Phase Does NOT Include

These are explicitly deferred to later phases:

- **Tool/function calling** (Phase 2) — no `tools` in the request, no tool-call loop
- **Sub-agents** (Phase 3) — single orchestrator only
- **File writes / diffs** (Phase 4) — read-only
- **TOML config** (Phase 5) — env vars and CLI flags only
- **Git integration** (Phase 6) — no branch awareness
- **Sandboxing** (Phase 7) — no platform restrictions
- **Session persistence** (Phase 8) — conversation is in-memory only
- **Full-screen TUI** (Phase 9) — line-based REPL only
- **Markdown rendering** (Phase 4) — plain text output for now

---

*See [phase_spec.md](phase_spec.md) for the full 10-phase roadmap and how this phase connects to subsequent phases.*

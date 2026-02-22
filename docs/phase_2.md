# Phase 2: Tool System + Filesystem Tools + Tool-Call Loop

**Goal**: The LLM can explore the codebase through Gemini function calling. Ask "What files are here?" and the model autonomously calls `list_directory`, gets results, and responds naturally.

**Deliverable**: `cargo run` launches the REPL where the model can read files, list directories, search for files by pattern, grep content, and execute allowlisted shell commands — all autonomously via the Gemini function calling protocol.

**Builds on**: Phase 1 (foundation, Gemini client, streaming REPL). All Phase 1 types and modules are extended, not replaced.

**Estimated**: ~2,000 lines of new Rust across ~4 new files + modifications to ~4 existing Phase 1 files.

---

## File Layout

### New Files

```
src/
  tool/
    mod.rs             # Tool trait (async_trait), FunctionDeclaration builder, module re-exports
    registry.rs        # ToolRegistry: register, get, execute, declarations_for_mode, to_gemini_tools()
    filesystem.rs      # ReadFileTool, ListDirectoryTool, SearchFilesTool, GrepTool
    shell.rs           # ShellCommandTool with command allowlist, shlex parsing, timeout
```

### Modified Phase 1 Files

```
src/
  gemini/
    types.rs           # + ToolDefinition, FunctionDeclaration, Parameters, ToolConfig, FunctionCallingConfig
                       # + tools/tool_config fields on GenerateContentRequest
  gemini/
    stream.rs          # Modify consume_stream return type to surface function call responses
  error.rs             # + ToolError, ShellError, ShellNotAllowed variants
  repl.rs              # Replace direct API call with run_tool_loop(); show tool usage in spinner
  lib.rs               # + pub mod tool;
```

---

## Dependencies to Add

Add these to the existing `Cargo.toml` from Phase 1:

```toml
# File search (glob patterns)
glob = "0.3"

# Regex content search
regex = "1"

# Gitignore-aware directory walking (used by ripgrep)
ignore = "0.4"

# Safe shell command string splitting
shlex = "1"
```

### New Crate Rationale

| Crate | Why |
|-------|-----|
| `glob` | Pattern matching for file search (`**/*.rs`, `src/*.toml`). Used by `SearchFilesTool`. |
| `regex` | Content search within files. Guaranteed linear-time matching (no ReDoS). Used by `GrepTool`. |
| `ignore` | Directory walker that respects `.gitignore`, `.ignore`, and hidden file conventions. Used by `ListDirectoryTool` and `SearchFilesTool` to avoid indexing `target/`, `node_modules/`, etc. Without this, the LLM would see thousands of irrelevant files. |
| `shlex` | Splits shell command strings into tokens respecting quotes and escapes. `"git log --oneline -10"` → `["git", "log", "--oneline", "-10"]`. Handles edge cases like `"git show 'message with spaces'"` correctly. Required for safe `tokio::process::Command` usage (explicit args, no `sh -c`). |

---

## Gemini Function Calling Reference

This section documents the Gemini function calling protocol that Phase 2 implements. Phase 1 defined the basic request/response types; Phase 2 adds the `tools` and `toolConfig` fields and implements the function calling cycle.

### Request with Tools

```json
{
  "contents": [ ... ],
  "systemInstruction": { ... },
  "generationConfig": { ... },
  "tools": [
    {
      "functionDeclarations": [
        {
          "name": "read_file",
          "description": "Read the contents of a file",
          "parameters": {
            "type": "object",
            "properties": {
              "path": {
                "type": "string",
                "description": "Path to the file to read"
              }
            },
            "required": ["path"]
          }
        }
      ]
    }
  ],
  "toolConfig": {
    "functionCallingConfig": {
      "mode": "AUTO"
    }
  }
}
```

**Key points**:
- `tools` is an array containing one object with a `functionDeclarations` array.
- Each declaration has `name`, `description`, and `parameters` (JSON Schema subset).
- `toolConfig.functionCallingConfig.mode` controls calling behavior:
  - `"AUTO"` (default) — model decides whether to call tools or respond with text.
  - `"ANY"` — model must call at least one tool.
  - `"NONE"` — tools disabled (equivalent to omitting `tools`).

### Supported JSON Schema Types

The `parameters` field supports a subset of OpenAPI 3.0 JSON Schema:

| Attribute | Supported |
|-----------|-----------|
| `type` | `string`, `integer`, `number`, `boolean`, `array`, `object` |
| `properties` | Object properties with nested schemas |
| `items` | Array element type |
| `required` | Array of required property names |
| `enum` | Fixed set of allowed values |
| `description` | Field documentation (strongly recommended) |
| `default` | **NOT supported** |
| `oneOf` / `anyOf` | **NOT supported** |

### Function Calling Cycle

```
1. User message → Gemini (with tool declarations)
2. Gemini responds with functionCall parts (finishReason is still "STOP")
3. Execute each function call locally
4. Build functionResponse parts, send back with role: "user"
5. Gemini responds with text (or more function calls)
6. Repeat until text-only response or max iterations
```

### Response with Function Calls

```json
{
  "candidates": [{
    "content": {
      "role": "model",
      "parts": [
        {
          "functionCall": {
            "name": "read_file",
            "args": { "path": "src/main.rs" }
          }
        }
      ]
    },
    "finishReason": "STOP"
  }]
}
```

**Critical**: `finishReason` is `"STOP"` even when function calls are present. Always inspect `content.parts` for `Part::FunctionCall` objects — do not rely on `finishReason`.

### Sending Function Responses Back

```json
{
  "contents": [
    { "role": "user", "parts": [{ "text": "What does main.rs do?" }] },
    {
      "role": "model",
      "parts": [
        { "functionCall": { "name": "read_file", "args": { "path": "src/main.rs" } } }
      ]
    },
    {
      "role": "user",
      "parts": [
        {
          "functionResponse": {
            "name": "read_file",
            "response": { "content": "fn main() { ... }" }
          }
        }
      ]
    }
  ]
}
```

**Rules**:
- Function responses use `role: "user"` — the user provides tool results back to the model.
- Each `functionResponse` is a separate `Part` within a single `Content` object.
- The `response` field is a JSON object (not a string).

### Parallel Function Calls

Gemini can return **multiple** `functionCall` parts in a single response:

```json
{
  "candidates": [{
    "content": {
      "parts": [
        { "functionCall": { "name": "read_file", "args": { "path": "src/main.rs" } } },
        { "functionCall": { "name": "read_file", "args": { "path": "Cargo.toml" } } }
      ]
    }
  }]
}
```

When this happens:
- Execute all function calls (can be done concurrently with `futures::future::join_all`).
- Return all results together in a single `Content` with `role: "user"`.
- Results are matched by position (order must correspond to the calls).

---

## Phase 1 Modifications

### `src/gemini/types.rs` — New Types

Add the following types after the existing `GenerationConfig` struct:

```rust
// ── Tool Definition Types (Phase 2) ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Parameters,
}

#[derive(Debug, Clone, Serialize)]
pub struct Parameters {
    #[serde(rename = "type")]
    pub schema_type: String, // "object"
    pub properties: serde_json::Map<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig {
    pub function_calling_config: FunctionCallingConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionCallingConfig {
    pub mode: String, // "AUTO", "ANY", "NONE"
}
```

### `src/gemini/types.rs` — Extend `GenerateContentRequest`

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    // Phase 2 additions:
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,
}
```

### `src/gemini/types.rs` — New Helper on `Content`

```rust
impl Content {
    // ... existing user(), model(), system() ...

    /// Build a Content with function response parts (role: "user").
    pub fn function_responses(responses: Vec<Part>) -> Self {
        Content {
            role: Some("user".into()),
            parts: responses,
        }
    }
}

impl GenerateContentResponse {
    // ... existing text() ...

    /// Extract all function call parts from the first candidate.
    pub fn function_calls(&self) -> Vec<&Part> {
        self.candidates.first()
            .and_then(|c| c.content.as_ref())
            .map(|content| {
                content.parts.iter()
                    .filter(|p| matches!(p, Part::FunctionCall { .. }))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether this response contains any function calls.
    pub fn has_function_calls(&self) -> bool {
        !self.function_calls().is_empty()
    }
}
```

### `src/gemini/stream.rs` — Modified Return Type

Change `consume_stream` to return function call responses so the REPL can hand off to the tool-call loop:

```rust
/// Result of consuming a stream.
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

/// Consume an EventSource and yield StreamEvents.
pub async fn consume_stream(
    mut es: EventSource,
    on_event: impl Fn(StreamEvent),
) -> Result<StreamResult> {
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
```

### `src/error.rs` — New Variants

```rust
#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // ... existing variants ...

    // Tool errors (Phase 2)
    #[error("Tool '{name}' not found in registry")]
    ToolNotFound { name: String },

    #[error("Tool '{name}' execution failed: {message}")]
    ToolError { name: String, message: String },

    #[error("Tool-call loop exceeded max iterations ({max})")]
    ToolLoopMaxIterations { max: usize },

    #[error("Command '{command}' is not in the allowlist. Allowed: {allowed}")]
    ShellNotAllowed { command: String, allowed: String },

    #[error("Shell command failed: {0}")]
    ShellError(String),

    #[error("Shell command timed out after {seconds}s")]
    ShellTimeout { seconds: u64 },

    #[error("File too large ({size_bytes} bytes, max {max_bytes}): {path}")]
    FileTooLarge { path: String, size_bytes: u64, max_bytes: u64 },

    #[error("Binary file detected: {path}")]
    BinaryFile { path: String },

    #[error("Invalid glob pattern: {0}")]
    GlobError(String),

    #[error("Invalid regex pattern: {0}")]
    RegexError(String),
}
```

---

## Implementation Details

### `src/tool/mod.rs`

The `Tool` trait is the core abstraction. Every tool implements this trait.

```rust
use async_trait::async_trait;
use serde_json::Value;
use std::fmt::Debug;
use crate::error::Result;
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;

/// A tool that the LLM can invoke via Gemini function calling.
#[async_trait]
pub trait Tool: Send + Sync + Debug {
    /// Unique name matching the Gemini function declaration.
    fn name(&self) -> &str;

    /// Human-readable description for the Gemini API.
    fn description(&self) -> &str;

    /// Generate the Gemini FunctionDeclaration for this tool.
    fn declaration(&self) -> FunctionDeclaration;

    /// Execute the tool with the given arguments (from Gemini's functionCall.args).
    /// Returns a JSON value that will be sent back as functionResponse.response.
    async fn execute(&self, args: Value) -> Result<Value>;

    /// Which modes this tool is available in.
    /// Default: all modes (Explore, Plan, Execute).
    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Explore, Mode::Plan, Mode::Execute]
    }
}

pub mod registry;
pub mod filesystem;
pub mod shell;
```

**Helper for building `FunctionDeclaration` parameter schemas**:

```rust
use serde_json::{json, Map, Value};
use crate::gemini::types::{FunctionDeclaration, Parameters};

/// Builder for FunctionDeclaration parameter schemas.
pub struct ParamBuilder {
    properties: Map<String, Value>,
    required: Vec<String>,
}

impl ParamBuilder {
    pub fn new() -> Self {
        Self {
            properties: Map::new(),
            required: Vec::new(),
        }
    }

    /// Add a string parameter.
    pub fn string(mut self, name: &str, description: &str, required: bool) -> Self {
        self.properties.insert(name.into(), json!({
            "type": "string",
            "description": description,
        }));
        if required {
            self.required.push(name.into());
        }
        self
    }

    /// Add an integer parameter.
    pub fn integer(mut self, name: &str, description: &str, required: bool) -> Self {
        self.properties.insert(name.into(), json!({
            "type": "integer",
            "description": description,
        }));
        if required {
            self.required.push(name.into());
        }
        self
    }

    /// Add a boolean parameter.
    pub fn boolean(mut self, name: &str, description: &str, required: bool) -> Self {
        self.properties.insert(name.into(), json!({
            "type": "boolean",
            "description": description,
        }));
        if required {
            self.required.push(name.into());
        }
        self
    }

    /// Build into Parameters.
    pub fn build(self) -> Parameters {
        Parameters {
            schema_type: "object".into(),
            properties: self.properties,
            required: if self.required.is_empty() {
                None
            } else {
                Some(self.required)
            },
        }
    }
}
```

### `src/tool/registry.rs`

Central registry that holds all tools and converts them to Gemini API format.

```rust
use std::collections::HashMap;
use serde_json::Value;
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::{ToolDefinition, ToolConfig, FunctionCallingConfig};
use crate::mode::Mode;
use super::Tool;

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Panics if a tool with the same name already exists.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        assert!(
            !self.tools.contains_key(&name),
            "Duplicate tool name: {name}"
        );
        self.tools.insert(name, tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Execute a tool by name with the given arguments.
    /// Returns the tool's response value, or an error if the tool
    /// is not found or execution fails.
    pub async fn execute(&self, name: &str, args: Value) -> Result<Value> {
        let tool = self.tools.get(name)
            .ok_or_else(|| ClosedCodeError::ToolNotFound {
                name: name.to_string(),
            })?;

        tracing::debug!("Executing tool '{}' with args: {}", name, args);

        tool.execute(args).await
    }

    /// Get function declarations for tools available in the given mode.
    pub fn declarations_for_mode(&self, mode: &Mode) -> Vec<crate::gemini::types::FunctionDeclaration> {
        self.tools.values()
            .filter(|tool| tool.available_modes().contains(mode))
            .map(|tool| tool.declaration())
            .collect()
    }

    /// Generate the `tools` array for a Gemini API request.
    /// Returns None if no tools are available for the given mode.
    pub fn to_gemini_tools(&self, mode: &Mode) -> Option<Vec<ToolDefinition>> {
        let declarations = self.declarations_for_mode(mode);
        if declarations.is_empty() {
            None
        } else {
            Some(vec![ToolDefinition {
                function_declarations: declarations,
            }])
        }
    }

    /// Generate the default `tool_config` for a Gemini API request.
    pub fn tool_config() -> ToolConfig {
        ToolConfig {
            function_calling_config: FunctionCallingConfig {
                mode: "AUTO".into(),
            },
        }
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}
```

**Registration helper** — builds the default registry with all Phase 2 tools:

```rust
use std::path::PathBuf;

/// Create a ToolRegistry with all Phase 2 tools registered.
pub fn create_default_registry(working_directory: PathBuf) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        super::filesystem::ReadFileTool::new(working_directory.clone())
    ));
    registry.register(Box::new(
        super::filesystem::ListDirectoryTool::new(working_directory.clone())
    ));
    registry.register(Box::new(
        super::filesystem::SearchFilesTool::new(working_directory.clone())
    ));
    registry.register(Box::new(
        super::filesystem::GrepTool::new(working_directory.clone())
    ));
    registry.register(Box::new(
        super::shell::ShellCommandTool::new(working_directory)
    ));
    registry
}
```

### `src/tool/filesystem.rs`

Four filesystem tools, each with a `working_directory` that all paths are resolved relative to.

#### `ReadFileTool`

Reads file contents with optional line range. Detects binary files. Truncates large files.

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;
use super::{Tool, ParamBuilder};

const MAX_FILE_SIZE: u64 = 100 * 1024; // 100KB
const NULL_BYTE_THRESHOLD: usize = 1; // Any null byte = binary

#[derive(Debug)]
pub struct ReadFileTool {
    working_directory: PathBuf,
}

impl ReadFileTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }

    /// Resolve a path relative to working_directory, preventing path traversal.
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let resolved = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.working_directory.join(path)
        };
        // Canonicalize to resolve .. and symlinks
        let canonical = resolved.canonicalize()
            .map_err(|e| ClosedCodeError::Io(e))?;
        Ok(canonical)
    }

    /// Check if file content appears to be binary (contains null bytes).
    fn is_binary(content: &[u8]) -> bool {
        let check_len = content.len().min(8192);
        content[..check_len].iter()
            .filter(|&&b| b == 0)
            .count() >= NULL_BYTE_THRESHOLD
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content with line numbers. \
         Supports optional start_line and end_line to read a specific range. \
         Large files (>100KB) are truncated with a warning."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("path", "Path to the file to read (relative to working directory)", true)
                .integer("start_line", "First line to read (1-indexed, inclusive). Omit to start from beginning.", false)
                .integer("end_line", "Last line to read (1-indexed, inclusive). Omit to read to end.", false)
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let path_str = args["path"].as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "read_file".into(),
                message: "Missing required parameter 'path'".into(),
            })?;

        let path = self.resolve_path(path_str)?;

        // Check file size
        let metadata = fs::metadata(&path).await
            .map_err(|e| ClosedCodeError::ToolError {
                name: "read_file".into(),
                message: format!("Cannot read '{}': {}", path_str, e),
            })?;

        let file_size = metadata.len();
        let truncated = file_size > MAX_FILE_SIZE;

        // Read file bytes
        let bytes = if truncated {
            let mut buf = vec![0u8; MAX_FILE_SIZE as usize];
            let mut file = fs::File::open(&path).await?;
            use tokio::io::AsyncReadExt;
            let n = file.read(&mut buf).await?;
            buf.truncate(n);
            buf
        } else {
            fs::read(&path).await
                .map_err(|e| ClosedCodeError::ToolError {
                    name: "read_file".into(),
                    message: format!("Cannot read '{}': {}", path_str, e),
                })?
        };

        // Binary detection
        if Self::is_binary(&bytes) {
            return Ok(json!({
                "error": format!("Binary file detected: {}", path_str),
                "file_size": file_size,
            }));
        }

        let content = String::from_utf8_lossy(&bytes);

        // Apply line range
        let start_line = args["start_line"].as_u64().map(|n| n as usize);
        let end_line = args["end_line"].as_u64().map(|n| n as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = start_line.unwrap_or(1).saturating_sub(1); // Convert to 0-indexed
        let end = end_line.unwrap_or(total_lines).min(total_lines);

        let selected: Vec<String> = lines[start..end].iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4}│ {}", start + i + 1, line))
            .collect();

        let output = selected.join("\n");

        let mut result = json!({
            "content": output,
            "path": path_str,
            "total_lines": total_lines,
            "lines_shown": format!("{}-{}", start + 1, end),
        });

        if truncated {
            result["warning"] = json!(format!(
                "File truncated: showing first {}KB of {}KB",
                MAX_FILE_SIZE / 1024,
                file_size / 1024,
            ));
        }

        Ok(result)
    }
}
```

#### `ListDirectoryTool`

Lists directory contents using the `ignore` crate for `.gitignore` awareness.

```rust
#[derive(Debug)]
pub struct ListDirectoryTool {
    working_directory: PathBuf,
}

impl ListDirectoryTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }
}

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str { "list_directory" }

    fn description(&self) -> &str {
        "List the contents of a directory. Returns file names, sizes, and types. \
         Respects .gitignore rules. Use recursive=true to list subdirectories."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("path", "Directory path (relative to working directory). Defaults to '.'", false)
                .boolean("recursive", "If true, list subdirectories recursively. Defaults to false.", false)
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let path_str = args["path"].as_str().unwrap_or(".");
        let recursive = args["recursive"].as_bool().unwrap_or(false);

        let dir_path = if Path::new(path_str).is_absolute() {
            PathBuf::from(path_str)
        } else {
            self.working_directory.join(path_str)
        };

        if !dir_path.is_dir() {
            return Ok(json!({
                "error": format!("Not a directory: {}", path_str),
            }));
        }

        // Use ignore crate's WalkBuilder for .gitignore awareness.
        // Run in blocking task since ignore::Walk is synchronous.
        let entries = tokio::task::spawn_blocking(move || {
            use ignore::WalkBuilder;

            let mut builder = WalkBuilder::new(&dir_path);
            if !recursive {
                builder.max_depth(Some(1));
            }
            builder.hidden(false) // show hidden files (user may want them)
                .git_ignore(true)  // respect .gitignore
                .git_global(true)  // respect global gitignore
                .git_exclude(true); // respect .git/info/exclude

            let mut entries = Vec::new();
            for entry in builder.build() {
                if let Ok(entry) = entry {
                    // Skip the root directory itself
                    if entry.path() == dir_path {
                        continue;
                    }

                    let relative = entry.path()
                        .strip_prefix(&dir_path)
                        .unwrap_or(entry.path())
                        .to_string_lossy()
                        .to_string();

                    let file_type = if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                        "directory"
                    } else {
                        "file"
                    };

                    let size = entry.metadata()
                        .map(|m| m.len())
                        .unwrap_or(0);

                    entries.push(json!({
                        "name": relative,
                        "type": file_type,
                        "size": size,
                    }));
                }
            }
            entries
        }).await.map_err(|e| ClosedCodeError::ToolError {
            name: "list_directory".into(),
            message: format!("Failed to list directory: {}", e),
        })?;

        Ok(json!({
            "path": path_str,
            "entries": entries,
            "count": entries.len(),
        }))
    }
}
```

#### `SearchFilesTool`

Glob-based file search using the `glob` crate, filtered through `ignore` for gitignore awareness.

```rust
#[derive(Debug)]
pub struct SearchFilesTool {
    working_directory: PathBuf,
}

impl SearchFilesTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }
}

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str { "search_files" }

    fn description(&self) -> &str {
        "Search for files matching a glob pattern (e.g., '**/*.rs', 'src/**/*.toml'). \
         Returns matching file paths relative to the working directory. Respects .gitignore."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("pattern", "Glob pattern to match files (e.g., '**/*.rs')", true)
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let pattern = args["pattern"].as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "search_files".into(),
                message: "Missing required parameter 'pattern'".into(),
            })?;

        let wd = self.working_directory.clone();
        let pattern = pattern.to_string();

        let matches = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let full_pattern = wd.join(&pattern).to_string_lossy().to_string();

            let paths: Vec<String> = glob::glob(&full_pattern)
                .map_err(|e| ClosedCodeError::GlobError(e.to_string()))?
                .filter_map(|entry| entry.ok())
                .filter_map(|path| {
                    path.strip_prefix(&wd)
                        .ok()
                        .map(|rel| rel.to_string_lossy().to_string())
                })
                .collect();

            Ok(paths)
        }).await.map_err(|e| ClosedCodeError::ToolError {
            name: "search_files".into(),
            message: format!("Search failed: {}", e),
        })??;

        Ok(json!({
            "pattern": args["pattern"],
            "matches": matches,
            "count": matches.len(),
        }))
    }
}
```

#### `GrepTool`

Regex content search across files. Returns matches with file path, line number, and context.

```rust
#[derive(Debug)]
pub struct GrepTool {
    working_directory: PathBuf,
}

const MAX_MATCHES: usize = 100; // Cap results to prevent huge responses

impl GrepTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str { "grep" }

    fn description(&self) -> &str {
        "Search file contents using a regex pattern. Returns matching lines with \
         file paths and line numbers. Optionally filter by file glob pattern. \
         Respects .gitignore. Results capped at 100 matches."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("pattern", "Regex pattern to search for", true)
                .string("file_pattern", "Optional glob to filter files (e.g., '*.rs'). Defaults to all files.", false)
                .boolean("case_insensitive", "If true, search case-insensitively. Defaults to false.", false)
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let pattern = args["pattern"].as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "grep".into(),
                message: "Missing required parameter 'pattern'".into(),
            })?;

        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
        let file_pattern = args["file_pattern"].as_str().map(|s| s.to_string());

        let regex_pattern = if case_insensitive {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };

        let re = regex::Regex::new(&regex_pattern)
            .map_err(|e| ClosedCodeError::RegexError(e.to_string()))?;

        let wd = self.working_directory.clone();

        let matches = tokio::task::spawn_blocking(move || -> Result<Vec<Value>> {
            use ignore::WalkBuilder;
            use std::io::{BufRead, BufReader};
            use std::fs::File;

            let mut results = Vec::new();

            let walker = WalkBuilder::new(&wd)
                .hidden(false)
                .git_ignore(true)
                .build();

            for entry in walker {
                if results.len() >= MAX_MATCHES {
                    break;
                }

                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                // Skip directories
                if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                    continue;
                }

                let path = entry.path();

                // Apply file pattern filter
                if let Some(ref fp) = file_pattern {
                    let file_name = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    if let Ok(glob_pattern) = glob::Pattern::new(fp) {
                        if !glob_pattern.matches(file_name) {
                            continue;
                        }
                    }
                }

                // Read and search file
                let file = match File::open(path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                let reader = BufReader::new(file);
                for (line_num, line) in reader.lines().enumerate() {
                    if results.len() >= MAX_MATCHES {
                        break;
                    }

                    let line = match line {
                        Ok(l) => l,
                        Err(_) => continue, // Skip binary/unreadable lines
                    };

                    if re.is_match(&line) {
                        let relative = path.strip_prefix(&wd)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .to_string();

                        results.push(json!({
                            "file": relative,
                            "line": line_num + 1,
                            "content": line.trim(),
                        }));
                    }
                }
            }

            Ok(results)
        }).await.map_err(|e| ClosedCodeError::ToolError {
            name: "grep".into(),
            message: format!("Search failed: {}", e),
        })??;

        let truncated = matches.len() >= MAX_MATCHES;

        Ok(json!({
            "pattern": args["pattern"],
            "matches": matches,
            "count": matches.len(),
            "truncated": truncated,
        }))
    }
}
```

### `src/tool/shell.rs`

Safe shell command execution with command allowlisting, `shlex` parsing, and timeout.

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;
use super::{Tool, ParamBuilder};

/// Commands allowed to be executed.
/// Read-only and informational commands only.
const ALLOWED_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "find", "grep", "rg",
    "wc", "file", "tree", "pwd", "which", "git",
    "cargo", "rustc", "echo", "sort", "uniq", "diff",
];

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct ShellCommandTool {
    working_directory: PathBuf,
}

impl ShellCommandTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }

    /// Parse a command string into (command, args) using shlex.
    /// Validates the command against the allowlist.
    fn parse_and_validate(command_str: &str) -> Result<(String, Vec<String>)> {
        let parts = shlex::split(command_str)
            .ok_or_else(|| ClosedCodeError::ShellError(
                "Invalid command syntax (mismatched quotes)".into()
            ))?;

        if parts.is_empty() {
            return Err(ClosedCodeError::ShellError("Empty command".into()));
        }

        let cmd = &parts[0];

        // Extract base command name (handle paths like /usr/bin/git)
        let base_cmd = std::path::Path::new(cmd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cmd);

        if !ALLOWED_COMMANDS.contains(&base_cmd) {
            return Err(ClosedCodeError::ShellNotAllowed {
                command: base_cmd.to_string(),
                allowed: ALLOWED_COMMANDS.join(", "),
            });
        }

        Ok((parts[0].clone(), parts[1..].to_vec()))
    }
}

#[async_trait]
impl Tool for ShellCommandTool {
    fn name(&self) -> &str { "shell" }

    fn description(&self) -> &str {
        "Execute a shell command. Only allowlisted commands are permitted: \
         ls, cat, head, tail, find, grep, rg, wc, file, tree, pwd, which, \
         git, cargo, rustc, echo, sort, uniq, diff. \
         Commands have a 30-second timeout. Use this for operations \
         not covered by other tools."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("command", "The shell command to execute (e.g., 'git log --oneline -10')", true)
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let command_str = args["command"].as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "shell".into(),
                message: "Missing required parameter 'command'".into(),
            })?;

        let (cmd, cmd_args) = Self::parse_and_validate(command_str)?;

        tracing::info!("Executing shell command: {} {:?}", cmd, cmd_args);

        // Execute with timeout
        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            Command::new(&cmd)
                .args(&cmd_args)
                .current_dir(&self.working_directory)
                .output()
        )
        .await
        .map_err(|_| ClosedCodeError::ShellTimeout { seconds: 30 })?
        .map_err(|e| ClosedCodeError::ShellError(
            format!("Failed to execute '{}': {}", cmd, e)
        ))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // Truncate very long output
        let max_output = 50_000; // 50KB
        let stdout_truncated = if stdout.len() > max_output {
            format!("{}...\n[Output truncated: {} bytes total]",
                &stdout[..max_output], stdout.len())
        } else {
            stdout
        };

        Ok(json!({
            "stdout": stdout_truncated,
            "stderr": stderr,
            "exit_code": exit_code,
            "command": command_str,
        }))
    }
}
```

---

## Tool-Call Loop

The core agentic loop. This is the most important control flow in the application. It lives as a function called from the REPL.

### `run_tool_loop` Function

Add this to a new file `src/tool_loop.rs` or inline in `repl.rs`:

```rust
use crate::gemini::client::GeminiClient;
use crate::gemini::types::*;
use crate::tool::registry::ToolRegistry;
use crate::error::{ClosedCodeError, Result};
use crate::ui::spinner::Spinner;

const MAX_TOOL_ITERATIONS: usize = 10;

/// Run the tool-call loop.
///
/// Sends the request to Gemini. If the response contains function calls,
/// executes them, sends the results back, and repeats. Continues until
/// Gemini responds with text only, or max iterations are reached.
///
/// Returns the final assistant text for appending to conversation history.
pub async fn run_tool_loop(
    client: &GeminiClient,
    registry: &ToolRegistry,
    history: &mut Vec<Content>,
    system_instruction: Option<Content>,
    generation_config: Option<GenerationConfig>,
    tools: Option<Vec<ToolDefinition>>,
    tool_config: Option<ToolConfig>,
    on_text: impl Fn(&str),     // Callback for streaming text display
    on_tool: impl Fn(&str),     // Callback for tool usage display (spinner message)
) -> Result<String> {
    let mut final_text = String::new();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        tracing::debug!("Tool loop iteration {}/{}", iteration + 1, MAX_TOOL_ITERATIONS);

        let request = GenerateContentRequest {
            contents: history.clone(),
            system_instruction: system_instruction.clone(),
            generation_config: generation_config.clone(),
            tools: tools.clone(),
            tool_config: tool_config.clone(),
        };

        // Use non-streaming for tool-call loop iterations.
        // Streaming is only used for the initial request (handled by REPL).
        let response = client.generate_content(&request).await?;

        // Check for empty response
        let candidate = response.candidates.first()
            .ok_or(ClosedCodeError::EmptyResponse)?;

        let content = candidate.content.as_ref()
            .ok_or(ClosedCodeError::EmptyResponse)?;

        // Check for safety block
        if candidate.finish_reason.as_deref() == Some("SAFETY") {
            let reason = candidate.safety_ratings.iter()
                .filter(|r| r.probability == "HIGH" || r.probability == "MEDIUM")
                .map(|r| r.category.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ClosedCodeError::SafetyBlocked { reason });
        }

        // Separate text parts from function call parts
        let mut text_parts = Vec::new();
        let mut function_calls = Vec::new();

        for part in &content.parts {
            match part {
                Part::Text(text) => text_parts.push(text.clone()),
                Part::FunctionCall { name, args } => {
                    function_calls.push((name.clone(), args.clone()));
                }
                _ => {}
            }
        }

        // Display any text
        if !text_parts.is_empty() {
            let text = text_parts.join("");
            on_text(&text);
            final_text.push_str(&text);
        }

        // If no function calls, we're done
        if function_calls.is_empty() {
            // Append the model's full Content to history
            history.push(content.clone());
            break;
        }

        // Append model's response (with function calls) to history
        history.push(content.clone());

        // Execute all function calls (can be done concurrently)
        let mut response_parts: Vec<Part> = Vec::new();

        for (name, args) in &function_calls {
            on_tool(&format!("Using {}...", name));

            let result = match registry.execute(name, args.clone()).await {
                Ok(value) => value,
                Err(e) => {
                    // Return error to the model, don't crash the loop
                    tracing::warn!("Tool '{}' failed: {}", name, e);
                    serde_json::json!({
                        "error": e.to_string(),
                    })
                }
            };

            response_parts.push(Part::FunctionResponse {
                name: name.clone(),
                response: result,
            });
        }

        // Append function responses to history (role: "user")
        history.push(Content::function_responses(response_parts));

        // Continue loop — Gemini will process the function results
    }

    // If we exhausted iterations without a text response
    if final_text.is_empty() {
        tracing::warn!("Tool loop exhausted {} iterations without final text", MAX_TOOL_ITERATIONS);
    }

    Ok(final_text)
}
```

### REPL Integration

Modify `src/repl.rs` to use the tool-call loop instead of direct API calls. The key change is in the message handling:

```rust
pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    let client = GeminiClient::new(config.api_key.clone(), config.model.clone());
    let registry = create_default_registry(config.working_directory.clone());
    let mut history: Vec<Content> = Vec::new();
    let mut editor = DefaultEditor::new()?;

    let system_prompt = build_system_prompt(config);
    let tools = registry.to_gemini_tools(&config.mode);
    let tool_config = if tools.is_some() {
        Some(ToolRegistry::tool_config())
    } else {
        None
    };

    // Print welcome
    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!("Mode: {} | Model: {} | Tools: {}",
        config.mode, config.model, registry.len());
    println!("Type /help for commands, /quit to exit.\n");

    loop {
        let prompt = format!("{} > ", config.mode);
        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() { continue; }
                editor.add_history_entry(line)?;

                if line.starts_with('/') {
                    match handle_slash_command(line, &mut history) {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                    }
                    continue;
                }

                // Build request
                history.push(Content::user(line));

                // First request: use streaming for immediate text display
                let spinner = Spinner::new("Thinking...");

                let request = GenerateContentRequest {
                    contents: history.clone(),
                    system_instruction: Some(Content::system(&system_prompt)),
                    generation_config: Some(GenerationConfig {
                        temperature: Some(1.0),
                        top_p: None,
                        top_k: None,
                        max_output_tokens: Some(config.max_output_tokens),
                    }),
                    tools: tools.clone(),
                    tool_config: tool_config.clone(),
                };

                let es = client.stream_generate_content(&request).await;
                spinner.finish();

                let stream_result = consume_stream(es, |event| {
                    match event {
                        StreamEvent::TextDelta(text) => {
                            print!("{}", text);
                            use std::io::Write;
                            std::io::stdout().flush().ok();
                        }
                        StreamEvent::Done { usage, .. } => {
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

                match stream_result {
                    Ok(StreamResult::Text(text)) => {
                        // Normal text response — done
                        println!();
                        history.push(Content::model(&text));
                    }
                    Ok(StreamResult::FunctionCall { text_so_far, response }) => {
                        // Function call detected — enter tool loop
                        if !text_so_far.is_empty() {
                            println!();
                        }

                        // Append model's function call response to history
                        if let Some(candidate) = response.candidates.first() {
                            if let Some(content) = &candidate.content {
                                history.push(content.clone());
                            }
                        }

                        // Extract and execute function calls
                        let mut response_parts = Vec::new();
                        for part in response.function_calls() {
                            if let Part::FunctionCall { name, args } = part {
                                let spinner = Spinner::new(
                                    &format!("Using {}...", name)
                                );

                                let result = match registry.execute(name, args.clone()).await {
                                    Ok(v) => v,
                                    Err(e) => {
                                        tracing::warn!("Tool '{}' failed: {}", name, e);
                                        serde_json::json!({"error": e.to_string()})
                                    }
                                };

                                spinner.finish();
                                response_parts.push(Part::FunctionResponse {
                                    name: name.clone(),
                                    response: result,
                                });
                            }
                        }

                        history.push(Content::function_responses(response_parts));

                        // Now continue with non-streaming tool loop
                        let spinner = Spinner::new("Thinking...");

                        let result = run_tool_loop(
                            &client,
                            &registry,
                            &mut history,
                            Some(Content::system(&system_prompt)),
                            Some(GenerationConfig {
                                temperature: Some(1.0),
                                top_p: None,
                                top_k: None,
                                max_output_tokens: Some(config.max_output_tokens),
                            }),
                            tools.clone(),
                            tool_config.clone(),
                            |text| {
                                print!("{}", text);
                                use std::io::Write;
                                std::io::stdout().flush().ok();
                            },
                            |msg| {
                                spinner.set_message(msg);
                            },
                        ).await;

                        spinner.finish();

                        match result {
                            Ok(_text) => {
                                println!();
                            }
                            Err(e) => {
                                eprintln!("\n{}: {}",
                                    styled_text("Error", Theme::ERROR), e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("\n{}: {}",
                            styled_text("Error", Theme::ERROR), e);
                    }
                }

                println!(); // spacing between turns
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
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
```

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **`ignore` crate** for directory walking | The `glob` crate has no `.gitignore` support. Without `ignore`, the LLM would see `target/`, `node_modules/`, `.git/` contents — thousands of irrelevant files that waste context and confuse the model. `ignore` is used by ripgrep and is battle-tested. |
| **`shlex`** for command parsing | Manually splitting on whitespace breaks quoted strings. `shlex` correctly handles `"git show 'message with spaces'"` → `["git", "show", "message with spaces"]`. Essential for safe `Command::new()` usage. |
| **Error-as-response** not crash | When a tool fails (file not found, regex invalid, command timeout), the error is returned to the model as a `functionResponse` with an `"error"` field. The model can then try a different approach or inform the user. Crashing the loop would break the conversation. |
| **Non-streaming** for tool loop | Tool call results are structured JSON, not human-readable text. The model needs the complete response to decide what to do next. Streaming is only used for the initial request (user-facing text). |
| **Explicit args** not `sh -c` | Passing the command string to `sh -c` enables shell injection (`"ls; rm -rf /"`). `Command::new("ls").arg("-la")` passes each argument directly to the process — no shell interpretation, no metacharacter expansion. |
| **Command allowlist** | Prevents destructive operations. Only read-only and informational commands are permitted. Write operations (`rm`, `mv`, `cp`, `mkdir`) are handled by dedicated tools in Phase 4, not the shell tool. |
| **100KB file size cap** | Sending multi-megabyte files to the LLM wastes context window and money. 100KB is enough for any source file. Binary files are detected (null byte check) and rejected entirely. |
| **`async_trait`** for Tool trait | The `Tool` trait needs `async fn execute()` for I/O-bound operations (file reads, process spawning). `async_trait` makes this work with `dyn Tool` in the registry HashMap. The performance cost (one heap allocation per call) is negligible compared to the API call latency. |
| **Max 10 iterations** | Prevents infinite loops if the model keeps calling tools without producing a final answer. 10 iterations is enough for most exploration tasks (read a file, grep for something, read another file, respond). Phase 3 raises this to 30 for the orchestrator. |
| **`spawn_blocking`** for `ignore::Walk` | The `ignore` crate's `Walk` iterator is synchronous. Wrapping it in `spawn_blocking` prevents blocking the tokio runtime. This matters because directory traversal can be slow on large repos. |

---

## Milestone / Verification

After implementing Phase 2, verify each capability:

```bash
# 1. Tools are registered and shown at startup
cargo run
# Expected:
#   closed-code
#   Mode: explore | Model: gemini-3.1-pro-preview | Tools: 5
#   Type /help for commands, /quit to exit.

# 2. LLM can list directory contents
# explore > What files are in this project?
# ⠋ Using list_directory...
# This project has the following files:
#   Cargo.toml, src/main.rs, src/cli.rs, ...
# (model synthesizes a natural response from the tool results)

# 3. LLM can read file contents
# explore > Show me the main function
# ⠋ Using read_file...
# Here's the main function from src/main.rs:
# (model shows and explains the code)

# 4. LLM can search for files
# explore > Find all Rust files
# ⠋ Using search_files...
# Found 12 .rs files in the project:
# src/main.rs, src/cli.rs, ...

# 5. LLM can grep for content
# explore > Search for TODO comments
# ⠋ Using grep...
# Found 3 TODOs:
#   src/client.rs:42 — // TODO: add retry for rate limits
#   ...

# 6. LLM can execute shell commands
# explore > Show me the git log
# ⠋ Using shell...
# Here are the recent commits:
#   abc1234 Initial commit
#   ...

# 7. Shell allowlist is enforced
# explore > Run rm -rf /
# (model calls shell tool with "rm -rf /")
# ⠋ Using shell...
# I tried to run that command but it's not allowed.
# The shell only permits: ls, cat, head, tail, ...
# (model receives error response and explains to user)

# 8. Multi-step tool usage (tool loop)
# explore > What does the error handling look like in this project?
# ⠋ Using search_files...
# ⠋ Using read_file...
# ⠋ Using grep...
# The error handling uses thiserror with a ClosedCodeError enum...
# (model chains multiple tool calls autonomously)

# 9. Large file handling
# explore > Read the compiled binary
# ⠋ Using read_file...
# That file appears to be binary (not a text file).
# (model receives binary detection error, explains to user)

# 10. Tool errors are handled gracefully
# explore > Read the file nonexistent.rs
# ⠋ Using read_file...
# That file doesn't exist. Let me search for similar files...
# ⠋ Using search_files...
# (model recovers from error by trying alternatives)

# 11. Verbose mode shows tool details
RUST_LOG=debug cargo run
# Expected: debug output shows "Executing tool 'read_file' with args: {...}"
#           and tool-call loop iteration counts

# 12. One-shot mode still works with tools
cargo run -- ask "What files are in this project?"
# Expected: streams response, may invoke tools, exits when done
```

---

## What This Phase Does NOT Include

These are explicitly deferred to later phases:

- **Sub-agents** (Phase 3) — no `SpawnExplorerTool`, single conversation only
- **File writes / edits** (Phase 4) — all tools are read-only
- **Diff display / approval gates** (Phase 4) — no `WriteFileTool`, no `ApprovalHandler`
- **TOML config** (Phase 5) — tool configuration is hardcoded
- **Git integration** (Phase 6) — `git` is available via shell tool but no deep integration
- **Sandboxing** (Phase 7) — shell commands run unsandboxed (allowlist is the only safety)
- **Session persistence** (Phase 8) — tool call history is in-memory only
- **Streaming during tool loop** (Phase 9/TUI) — tool loop uses non-streaming API
- **Parallel tool execution** — function calls are executed sequentially for simplicity. Concurrent execution (via `futures::future::join_all`) is a Phase 10 optimization.

---

*See [phase_1.md](phase_1.md) for the foundation this phase builds on, and [phase_spec.md](phase_spec.md) for the full 10-phase roadmap.*

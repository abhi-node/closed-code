# Phase 8b: Image Support + MCP Client

**Goal**: Enable image understanding via clipboard paste and file reading, with a description sub-agent pipeline. Add Model Context Protocol (MCP) client support for dynamically discovering and invoking external tools over STDIO transport.

**Depends on**: Phase 5 (Configuration + Enhanced REPL)

---

## Phase Dependency Graph (within Phase 8b)

```
8b.1 Image Utilities (clipboard, file read, base64, MIME)
  │
  └──► 8b.2 ImageDescriptionAgent (sub-agent for image → text)
              │
              ├──► 8b.3 ReadImageTool (LLM-callable)
              │
              └──► 8b.4 /paste Command + --image CLI Flag

8b.5 JSON-RPC 2.0 Types (request, response, notification, error)
  │
  └──► 8b.6 StdioTransport (spawn, handshake, tool discovery, call_tool, shutdown)
              │
              └──► 8b.7 McpToolProxy (dynamic Box<dyn Tool> wrapper)
                        │
                        └──► 8b.8 Config + Startup Integration ([mcp_servers], /mcp)
```

---

## Files Overview

```
src/
  image/
    mod.rs             # NEW: Image module — clipboard reading, file reading, base64, MIME detection
  agent/
    mod.rs             # MODIFIED: Add pub mod image_description;
    image_description.rs  # NEW: ImageDescriptionAgent — receives image + prompt, returns text description
  tool/
    mod.rs             # MODIFIED: Add pub mod image;
    image.rs           # NEW: ReadImageTool — LLM-callable tool for reading image files
    registry.rs        # MODIFIED: Register read_image + MCP tools in factory functions
  mcp/
    mod.rs             # NEW: MCP module root — re-exports, McpServer struct, lifecycle functions
    jsonrpc.rs         # NEW: JSON-RPC 2.0 types (Request, Response, Notification, Error)
    transport.rs       # NEW: StdioTransport — child process management, message framing
    proxy.rs           # NEW: McpToolProxy — wraps an MCP tool as Box<dyn Tool>
  config.rs            # MODIFIED: Add McpServerConfig, [mcp_servers] TOML section
  cli.rs               # MODIFIED: Add --image flag on Ask subcommand
  error.rs             # MODIFIED: Add ImageError, McpError variants
  repl.rs              # MODIFIED: Add /paste command, /mcp command, --image oneshot support
  lib.rs               # MODIFIED: Add pub mod image; pub mod mcp;
  agent/
    orchestrator.rs    # MODIFIED: Add image context injection, MCP tool registration
  main.rs              # MODIFIED: Pass --image to run_oneshot
```

### New Cargo Dependencies

```toml
[dependencies]
# Clipboard access (cross-platform, by 1Password)
arboard = "3"

# Base64 encoding for image data
base64 = "0.22"

# PNG encoding for clipboard RGBA → PNG conversion
png = "0.17"
```

No additional platform-specific dependencies. `arboard` handles macOS (NSPasteboard), Linux (X11/Wayland), and Windows natively. `base64` and `png` are pure Rust.

---

## Sub-Phase 8b.1: Image Utilities

### New File: `src/image/mod.rs`

Module providing clipboard image reading, file image reading, base64 encoding, and MIME type detection.

```rust
use std::path::Path;

use base64::Engine;

use crate::error::{ClosedCodeError, Result};
```

**ImageMimeType — supported formats:**

```rust
/// Supported image MIME types for Gemini's InlineData.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageMimeType {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageMimeType {
    /// MIME type string for the Gemini API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    /// Detect MIME type from file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    /// Detect MIME type from magic bytes (first 12 bytes of file).
    pub fn from_magic_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF8") {
            Some(Self::Gif)
        } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }
}

impl std::fmt::Display for ImageMimeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
```

**ImageData — image ready for API submission:**

```rust
/// An image ready for submission to the Gemini API.
#[derive(Debug, Clone)]
pub struct ImageData {
    /// Base64-encoded image bytes.
    pub base64_data: String,
    /// MIME type of the image.
    pub mime_type: ImageMimeType,
    /// Original source description (for logging/display).
    pub source: String,
    /// Size in bytes of the raw (pre-base64) image data.
    pub size_bytes: usize,
}

/// Maximum image size: 20 MB (Gemini's inline data limit).
const MAX_IMAGE_SIZE: usize = 20 * 1024 * 1024;
```

**Clipboard image reading:**

```rust
/// Read an image from the system clipboard.
///
/// Uses `arboard` to access clipboard image data. Clipboard images are
/// returned as raw RGBA pixels by arboard, which we encode to PNG before
/// base64-encoding for the Gemini API.
pub fn read_clipboard_image() -> Result<ImageData> {
    use arboard::Clipboard;

    let mut clipboard = Clipboard::new().map_err(|e| {
        ClosedCodeError::ImageError(format!("Failed to access clipboard: {}", e))
    })?;

    let image = clipboard.get_image().map_err(|e| {
        ClosedCodeError::ImageError(format!(
            "No image in clipboard ({}). Copy an image first.",
            e,
        ))
    })?;

    // arboard returns raw RGBA bytes. Encode to PNG for Gemini.
    let png_bytes = encode_rgba_to_png(
        &image.bytes,
        image.width as u32,
        image.height as u32,
    )?;

    if png_bytes.len() > MAX_IMAGE_SIZE {
        return Err(ClosedCodeError::ImageError(format!(
            "Clipboard image too large ({} bytes, max {} bytes)",
            png_bytes.len(),
            MAX_IMAGE_SIZE,
        )));
    }

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

    Ok(ImageData {
        size_bytes: png_bytes.len(),
        base64_data,
        mime_type: ImageMimeType::Png,
        source: "clipboard".into(),
    })
}
```

**File image reading:**

```rust
/// Read an image from a file path.
///
/// Detects MIME type from magic bytes (preferred) and file extension (fallback).
/// Returns base64-encoded image data ready for the Gemini API.
pub fn read_image_file(path: &Path) -> Result<ImageData> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let bytes = std::fs::read(path).map_err(|e| {
        ClosedCodeError::ImageError(format!(
            "Failed to read image file '{}': {}",
            path.display(),
            e,
        ))
    })?;

    if bytes.len() > MAX_IMAGE_SIZE {
        return Err(ClosedCodeError::ImageError(format!(
            "Image file too large ({} bytes, max {} bytes): {}",
            bytes.len(),
            MAX_IMAGE_SIZE,
            path.display(),
        )));
    }

    // Detect MIME type: prefer magic bytes, fall back to extension
    let mime_type = ImageMimeType::from_magic_bytes(&bytes)
        .or_else(|| ImageMimeType::from_extension(ext))
        .ok_or_else(|| {
            ClosedCodeError::ImageError(format!(
                "Unsupported image format: {}. Supported: PNG, JPEG, GIF, WebP",
                path.display(),
            ))
        })?;

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(ImageData {
        size_bytes: bytes.len(),
        base64_data,
        mime_type,
        source: path.display().to_string(),
    })
}
```

**PNG encoding helper:**

```rust
/// Encode raw RGBA pixel data to PNG bytes.
///
/// arboard returns clipboard images as raw RGBA pixel buffers. We need
/// to encode them as PNG for the Gemini API's InlineData format.
fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    use std::io::Cursor;

    let mut buf = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut buf), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| {
            ClosedCodeError::ImageError(format!("PNG encoding failed: {}", e))
        })?;
        writer.write_image_data(rgba).map_err(|e| {
            ClosedCodeError::ImageError(format!("PNG encoding failed: {}", e))
        })?;
    }
    Ok(buf)
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| `arboard` for clipboard | Cross-platform (macOS, Linux, Windows), maintained by 1Password, lightweight. Alternatives like `clipboard` are unmaintained. |
| PNG as clipboard output format | arboard returns raw RGBA pixels. PNG is lossless, universally supported by Gemini, and keeps the pipeline simple. |
| Magic byte detection as primary | More reliable than file extensions (users may rename files). Extension is the fallback. |
| 20 MB size limit | Matches Gemini's inline data limit. Checked before base64 encoding to fail fast. |
| `ImageData` struct bundles everything | base64 + MIME + metadata. Passed to the description agent and the API as `Part::InlineData`. |
| Separate `read_clipboard_image()` and `read_image_file()` | Different code paths: clipboard uses arboard + RGBA-to-PNG; file uses fs::read + magic bytes. |
| `png` crate dependency | Minimal, pure Rust PNG encoder. Needed because arboard returns raw RGBA on some platforms. |

### `src/lib.rs` — Modified

Add `pub mod image;` to module declarations.

---

## Sub-Phase 8b.2: ImageDescriptionAgent

### New File: `src/agent/image_description.rs`

A sub-agent that receives an image (as `Part::InlineData`) plus the user's current prompt, and produces a detailed text description. This description is then injected into the main orchestrator's context.

```rust
use std::sync::Arc;
use std::time::Duration;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::*;
use crate::gemini::GeminiClient;
use crate::image::ImageData;

const IMAGE_DESCRIPTION_TIMEOUT_SECS: u64 = 60;

const IMAGE_DESCRIPTION_SYSTEM_PROMPT: &str = "\
You are an expert image analyst. Your job is to provide a thorough, detailed \
description of the image provided to you.

Your description must be comprehensive and cover:
1. What the image shows (UI screenshot, diagram, error message, code, chart, etc.)
2. All visible text (transcribe it exactly)
3. Layout, structure, and spatial relationships
4. Colors, styling, and visual hierarchy
5. Any notable details that would help someone who cannot see the image

The user has also provided their current prompt/question. Tailor your description \
to be maximally useful in the context of their question. If the image shows code, \
transcribe the code. If it shows an error, include the full error text. If it shows \
a UI, describe the UI elements and their state.

Be thorough but organized. Use markdown formatting for structure.";
```

**Agent struct and `describe()` method:**

```rust
/// A sub-agent that describes an image in the context of the user's prompt.
///
/// Unlike other agents, this one does NOT have tools. It receives the image
/// as InlineData in a single API call and returns a text description.
#[derive(Debug)]
pub struct ImageDescriptionAgent;

impl ImageDescriptionAgent {
    pub fn new() -> Self {
        Self
    }

    /// Describe an image in the context of a user prompt.
    ///
    /// Sends a single API call with the image (InlineData) + prompt text
    /// and returns the model's text description.
    pub async fn describe(
        &self,
        client: &GeminiClient,
        image: &ImageData,
        user_prompt: &str,
    ) -> Result<String> {
        let user_content = Content {
            role: Some("user".into()),
            parts: vec![
                Part::InlineData {
                    mime_type: image.mime_type.as_str().to_string(),
                    data: image.base64_data.clone(),
                },
                Part::Text(format!(
                    "Describe this image in detail. The user's current question/context is:\n\n{}",
                    user_prompt,
                )),
            ],
        };

        let request = GenerateContentRequest {
            contents: vec![user_content],
            system_instruction: Some(Content::system(IMAGE_DESCRIPTION_SYSTEM_PROMPT)),
            generation_config: Some(GenerationConfig {
                temperature: Some(0.4),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(4096),
            }),
            tools: None,
            tool_config: None,
        };

        let response = tokio::time::timeout(
            Duration::from_secs(IMAGE_DESCRIPTION_TIMEOUT_SECS),
            client.generate_content(&request),
        )
        .await
        .map_err(|_| ClosedCodeError::AgentTimeout {
            agent_id: "image_description".into(),
            seconds: IMAGE_DESCRIPTION_TIMEOUT_SECS,
        })??;

        let text = response.text().unwrap_or("").to_string();

        if text.is_empty() {
            return Err(ClosedCodeError::AgentError {
                agent_id: "image_description".into(),
                message: "Image description agent returned empty response".into(),
            });
        }

        Ok(text)
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| Single API call, no tool loop | Image description is a focused task. The model sees the image + prompt and produces text. No tools needed — this is pure vision. |
| `describe()` standalone method | The standard `Agent::run()` takes `AgentRequest` which has no image field. `describe()` takes `ImageData` directly for a clean API. |
| User prompt included with image | Critical for context. "Describe this screenshot" vs "What's the error in this screenshot?" produce very different, more useful descriptions. |
| Temperature 0.4 | Lower temperature for accurate, factual description. Not creative writing — we want precision. |
| 4096 max output tokens | Image descriptions can be long (especially for screenshots with lots of text/code), but 4096 is sufficient. |
| 60-second timeout | Image processing by the model takes longer than text-only, but 60s is generous even for complex images. |
| No `Agent` trait impl | This agent doesn't follow the standard agent pattern (no tool loop, no AgentRequest). It's a focused helper, not a general-purpose agent. |

### `src/agent/mod.rs` — Modified

Add `pub mod image_description;` to the module declarations.

---

## Sub-Phase 8b.3: ReadImageTool

### New File: `src/tool/image.rs`

An LLM-callable tool that reads an image file from disk, sends it to the `ImageDescriptionAgent`, and returns the text description as the tool result.

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::image_description::ImageDescriptionAgent;
use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::FunctionDeclaration;
use crate::gemini::GeminiClient;
use crate::image;
use crate::mode::Mode;

use super::{ParamBuilder, Tool};

#[derive(Debug)]
pub struct ReadImageTool {
    client: Arc<GeminiClient>,
    working_directory: PathBuf,
}

impl ReadImageTool {
    pub fn new(client: Arc<GeminiClient>, working_directory: PathBuf) -> Self {
        Self {
            client,
            working_directory,
        }
    }

    /// Resolve a path relative to the working directory.
    fn resolve_path(&self, path_str: &str) -> PathBuf {
        let path = Path::new(path_str);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.working_directory.join(path)
        }
    }
}

#[async_trait]
impl Tool for ReadImageTool {
    fn name(&self) -> &str {
        "read_image"
    }

    fn description(&self) -> &str {
        "Read an image file and get a detailed text description of its contents. \
         Supports PNG, JPEG, GIF, and WebP formats. The image is analyzed by a \
         vision model that describes what it sees, including any text, code, UI \
         elements, diagrams, or other visual content. Use this when you need to \
         understand the contents of an image file."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "path",
                    "Path to the image file (absolute or relative to working directory). \
                     Supports: .png, .jpg, .jpeg, .gif, .webp",
                    true,
                )
                .string(
                    "context",
                    "Optional context about what you are looking for in the image. \
                     This helps the vision model focus on relevant details.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "read_image".into(),
                message: "Missing required parameter 'path'".into(),
            })?;
        let context = args["context"]
            .as_str()
            .unwrap_or("Describe this image in detail.");

        let resolved_path = self.resolve_path(path_str);

        // Check file exists
        if !resolved_path.exists() {
            return Err(ClosedCodeError::ToolError {
                name: "read_image".into(),
                message: format!("Image file not found: {}", resolved_path.display()),
            });
        }

        // Read and encode the image
        let image_data = image::read_image_file(&resolved_path)?;

        // Get description from the image description agent
        let agent = ImageDescriptionAgent::new();
        let description = agent.describe(&self.client, &image_data, context).await?;

        Ok(json!({
            "path": path_str,
            "mime_type": image_data.mime_type.as_str(),
            "size_bytes": image_data.size_bytes,
            "description": description,
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Explore, Mode::Plan, Mode::Guided, Mode::Execute, Mode::Auto]
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| Tool returns text description, not raw image | The main LLM context is text-based. Injecting base64 image data into conversation history would waste context tokens. The description agent extracts the information as text. |
| `context` parameter is optional | Allows the LLM to specify what it's looking for, improving description relevance. Defaults to generic description. |
| Available in all modes | Image reading is a read-only operation. Safe for Explore through Auto. |
| Path resolution relative to working_directory | Consistent with ReadFileTool and other filesystem tools. |
| `Arc<GeminiClient>` dependency | Needed to call the ImageDescriptionAgent, which makes its own API call to Gemini. |

### `src/tool/mod.rs` — Modified

Add `pub mod image;` to the module declarations.

### `src/tool/registry.rs` — Modified

**Updated `register_filesystem_tools()` to accept optional client for read_image:**

```rust
fn register_filesystem_tools(
    registry: &mut ToolRegistry,
    working_directory: PathBuf,
    bypass_shell_allowlist: bool,
    sandbox: Arc<dyn Sandbox>,
    client: Option<Arc<GeminiClient>>,  // NEW: needed for read_image
) {
    registry.register(Box::new(ReadFileTool::new(working_directory.clone())));
    registry.register(Box::new(ListDirectoryTool::new(working_directory.clone())));
    registry.register(Box::new(SearchFilesTool::new(working_directory.clone())));
    registry.register(Box::new(GrepTool::new(working_directory.clone())));

    if bypass_shell_allowlist {
        registry.register(Box::new(
            ShellCommandTool::with_bypass_allowlist(working_directory.clone(), sandbox),
        ));
    } else {
        registry.register(Box::new(ShellCommandTool::new(
            working_directory.clone(),
            sandbox,
        )));
    }

    // Image tool (needs GeminiClient for description sub-agent)
    if let Some(client) = client {
        registry.register(Box::new(super::image::ReadImageTool::new(
            client,
            working_directory,
        )));
    }
}
```

**Updated `create_default_registry()`:**

```rust
pub fn create_default_registry(
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_filesystem_tools(&mut registry, working_directory, false, sandbox, None);
    registry
}
```

**Updated `create_subagent_registry()`:**

```rust
pub fn create_subagent_registry(
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_filesystem_tools(&mut registry, working_directory, false, sandbox, None);
    registry.register(Box::new(CreateReportTool));
    registry
}
```

Sub-agents do NOT get `read_image` — they don't need image capabilities and passing GeminiClient to sub-agent registries adds complexity.

**Updated `create_orchestrator_registry()`:**

```rust
pub fn create_orchestrator_registry(
    working_directory: PathBuf,
    mode: &Mode,
    client: Arc<GeminiClient>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    sandbox: Arc<dyn Sandbox>,
    protected_paths: Vec<String>,
) -> ToolRegistry {
    let bypass_shell = matches!(mode, Mode::Auto);
    let mut registry = ToolRegistry::new();
    register_filesystem_tools(
        &mut registry,
        working_directory.clone(),
        bypass_shell,
        sandbox.clone(),
        Some(client.clone()),  // Pass client for read_image
    );

    // ... rest unchanged (spawn tools, write tools) ...

    registry
}
```

**New `tool_names()` method:**

```rust
impl ToolRegistry {
    /// Get all registered tool names (sorted for display).
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }
}
```

---

## Sub-Phase 8b.4: /paste Slash Command + --image CLI Flag

### `src/repl.rs` — Modified

**New `/paste` slash command:**

```rust
"/paste" => {
    let user_prompt = if arg.is_empty() {
        "Describe this image."
    } else {
        arg
    };

    println!("Reading clipboard image...");
    match crate::image::read_clipboard_image() {
        Ok(image_data) => {
            println!(
                "\u{2713} Image from clipboard ({}, {} bytes)",
                image_data.mime_type,
                image_data.size_bytes,
            );

            println!("Analyzing image...");
            let agent = crate::agent::image_description::ImageDescriptionAgent::new();
            let client = orchestrator.client();
            match agent.describe(&client, &image_data, user_prompt).await {
                Ok(description) => {
                    // Inject the image description into the orchestrator's context
                    let context_message = format!(
                        "[IMAGE from clipboard]\n\
                         User's question: {}\n\n\
                         Image description:\n{}",
                        user_prompt, description,
                    );
                    orchestrator.inject_image_context(&context_message);

                    println!("{}", description);
                    println!("\n(Image description added to context)");
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
        }
    }
    SlashResult::Continue
}
```

**Updated `/help`:**

```
/paste [prompt]      — Paste image from clipboard and describe it
/mcp                 — Show connected MCP servers and their tools
```

### `src/agent/orchestrator.rs` — Modified

**New methods for image support:**

```rust
impl Orchestrator {
    /// Inject an image description into conversation history as a user message.
    ///
    /// Called by /paste after the ImageDescriptionAgent produces a description.
    /// The description becomes part of the conversation context so the LLM
    /// can reference it in subsequent messages.
    pub fn inject_image_context(&mut self, context: &str) {
        self.history.push(Content::user(context));

        // Emit session event if session persistence is active (Phase 8a)
        // self.emit_event(SessionEvent::ImageAttached { ... });
    }

    /// Get a clone of the Arc<GeminiClient> (for sub-agent use outside orchestrator).
    pub fn client(&self) -> Arc<GeminiClient> {
        self.client.clone()
    }
}
```

### `src/cli.rs` — Modified

**Updated `Commands::Ask` subcommand:**

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Send a one-shot question (non-interactive)
    Ask {
        /// The question to ask
        question: String,

        /// Path to an image file to include with the question
        #[arg(long, value_name = "PATH")]
        image: Option<String>,
    },

    // ... other subcommands ...
}
```

### `src/repl.rs` — Modified (`run_oneshot`)

**Updated `run_oneshot` signature and image handling:**

```rust
pub async fn run_oneshot(
    config: &Config,
    question: &str,
    image_path: Option<&str>,
) -> anyhow::Result<()> {
    // ... existing client + orchestrator setup ...

    // If an image path is provided, describe it and prepend to the question
    let final_question = if let Some(path) = image_path {
        let image_data = crate::image::read_image_file(std::path::Path::new(path))?;

        println!("Analyzing image...");
        let agent = crate::agent::image_description::ImageDescriptionAgent::new();
        let description = agent.describe(&client, &image_data, question).await?;

        format!(
            "[IMAGE from {}]\n\nImage description:\n{}\n\nUser question: {}",
            path, description, question,
        )
    } else {
        question.to_string()
    };

    match orchestrator
        .handle_user_input_streaming(&final_question, /* callback */)
        .await
    {
        // ... unchanged ...
    }

    Ok(())
}
```

### `src/main.rs` — Modified

**Updated ask subcommand dispatch:**

```rust
match &cli.command {
    Some(Commands::Ask { question, image }) => {
        run_oneshot(&config, question, image.as_deref()).await?;
    }
    None => {
        run_repl(&config).await?;
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| `/paste` injects description as `Content::user()` | The main LLM sees the description as user context. Raw image bytes never enter the main conversation history — saves tokens. |
| `/paste [prompt]` accepts optional prompt | Lets the user specify context: `/paste What error is shown?` vs just `/paste`. |
| `--image` on `ask` subcommand only | One-shot mode. The REPL uses `/paste` instead. Keeps the CLI clean. |
| Image description text, not raw InlineData in history | Gemini charges per-image-token. Sending the full image every turn would be expensive and wasteful. The text description is sufficient for follow-up questions. |
| `inject_image_context()` method | Clean separation. The orchestrator doesn't know about images directly; it just gets text context injected. |

---

## Sub-Phase 8b.5: JSON-RPC 2.0 Types

### New File: `src/mcp/jsonrpc.rs`

Complete JSON-RPC 2.0 types for MCP communication, plus MCP-specific message types.

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 version string.
pub const JSONRPC_VERSION: &str = "2.0";
```

**Core JSON-RPC types:**

```rust
/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response (may contain result or error).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 notification (no id, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 request/response ID. Can be a number or a string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    Str(String),
}

impl JsonRpcId {
    pub fn new(id: i64) -> Self {
        Self::Number(id)
    }
}

impl std::fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{}", n),
            Self::Str(s) => write!(f, "{}", s),
        }
    }
}

/// Standard JSON-RPC 2.0 error codes.
pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}
```

**MCP-specific types:**

```rust
// ── MCP Initialize ──

/// MCP initialize request params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// MCP initialize response result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: Option<ToolsCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCapability {
    #[serde(default)]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: Option<String>,
}

// ── MCP Tools ──

/// MCP tools/list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<McpToolDefinition>,
}

/// An MCP tool definition (from tools/list).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// MCP tools/call request params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// MCP tools/call response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: Option<bool>,
}

/// MCP content item (text, image, or resource).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource { resource: McpResource },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
}
```

**Constructors:**

```rust
impl JsonRpcRequest {
    pub fn new(id: i64, method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id: JsonRpcId::new(id),
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcNotification {
    pub fn new(method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcResponse {
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}
```

**JSON-RPC message flow examples:**

```
Client → Server (initialize):
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"closed-code","version":"0.1.0"}}}

Server → Client (initialize response):
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"my-server","version":"1.0.0"}}}

Client → Server (initialized notification):
{"jsonrpc":"2.0","method":"notifications/initialized"}

Client → Server (tools/list):
{"jsonrpc":"2.0","id":2,"method":"tools/list"}

Server → Client (tools/list response):
{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"get_weather","description":"Get weather","inputSchema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}]}}

Client → Server (tools/call):
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_weather","arguments":{"city":"London"}}}

Server → Client (tools/call response):
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"London: 15°C, cloudy"}]}}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| `JsonRpcId` as `Number \| Str` | JSON-RPC 2.0 spec allows both. MCP servers may use either. `#[serde(untagged)]` handles it. |
| Separate `Request`, `Response`, `Notification` types | Cleaner than a single union type. MCP messages are always unambiguous in direction. |
| MCP-specific types alongside generic JSON-RPC | `InitializeParams`, `ToolsListResult`, etc. are MCP protocol types layered on JSON-RPC. Co-locating avoids fragmentation. |
| `McpContent` as `#[serde(tag = "type")]` | MCP content items are `{"type": "text", "text": "..."}`. Serde's tag attribute handles this naturally. |
| `Value` for `input_schema` | MCP tool schemas are JSON Schema objects. We pass them through as `Value` rather than strongly-typing JSON Schema. |

---

## Sub-Phase 8b.6: StdioTransport

### New File: `src/mcp/transport.rs`

Manages the lifecycle of an MCP server process: spawn, initialize handshake, tool discovery, tool invocation, and shutdown.

```rust
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::error::{ClosedCodeError, Result};
use super::jsonrpc::*;

const INITIALIZE_TIMEOUT_SECS: u64 = 30;
const CALL_TIMEOUT_SECS: u64 = 120;
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
```

**StdioTransport struct:**

```rust
/// A connection to an MCP server over STDIO transport.
///
/// Spawns the server process, manages the JSON-RPC 2.0 message exchange
/// over stdin/stdout, and provides methods for the MCP lifecycle:
/// initialize → tools/list → tools/call → shutdown.
#[derive(Debug)]
pub struct StdioTransport {
    /// The server process.
    child: Mutex<Child>,
    /// Stdin writer for sending requests.
    writer: Mutex<tokio::process::ChildStdin>,
    /// Stdout reader for receiving responses.
    reader: Mutex<BufReader<tokio::process::ChildStdout>>,
    /// Monotonically increasing request ID.
    next_id: AtomicI64,
    /// Server info from the initialize response.
    server_info: Mutex<Option<ServerInfo>>,
    /// Server name (from config key).
    server_name: String,
}
```

**Spawn:**

```rust
impl StdioTransport {
    /// Spawn a new MCP server process and return a transport handle.
    ///
    /// Does NOT perform the initialize handshake — call `initialize()` after.
    pub async fn spawn(
        server_name: &str,
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(env_vars) = env {
            for (k, v) in env_vars {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            ClosedCodeError::McpError(format!(
                "Failed to spawn MCP server '{}' (command: {}): {}",
                server_name, command, e,
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ClosedCodeError::McpError(format!("MCP server '{}' has no stdin", server_name))
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ClosedCodeError::McpError(format!("MCP server '{}' has no stdout", server_name))
        })?;

        Ok(Self {
            child: Mutex::new(child),
            writer: Mutex::new(stdin),
            reader: Mutex::new(BufReader::new(stdout)),
            next_id: AtomicI64::new(1),
            server_info: Mutex::new(None),
            server_name: server_name.to_string(),
        })
    }

    /// Get the server name.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Get the server info (available after initialize).
    pub async fn server_info(&self) -> Option<ServerInfo> {
        self.server_info.lock().await.clone()
    }
}
```

**Initialize handshake:**

```rust
impl StdioTransport {
    /// Perform the MCP initialize handshake.
    ///
    /// Sends `initialize` request → waits for response → sends `initialized` notification.
    pub async fn initialize(&self) -> Result<InitializeResult> {
        let params = InitializeParams {
            protocol_version: MCP_PROTOCOL_VERSION.into(),
            capabilities: ClientCapabilities {},
            client_info: ClientInfo {
                name: "closed-code".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        };

        let response = tokio::time::timeout(
            Duration::from_secs(INITIALIZE_TIMEOUT_SECS),
            self.send_request("initialize", Some(serde_json::to_value(&params).unwrap())),
        )
        .await
        .map_err(|_| {
            ClosedCodeError::McpError(format!(
                "MCP server '{}' initialize timed out after {}s",
                self.server_name, INITIALIZE_TIMEOUT_SECS,
            ))
        })??;

        let result: InitializeResult = serde_json::from_value(
            response.result.ok_or_else(|| {
                ClosedCodeError::McpError(format!(
                    "MCP server '{}' initialize returned error: {:?}",
                    self.server_name, response.error,
                ))
            })?,
        )
        .map_err(|e| {
            ClosedCodeError::McpError(format!(
                "MCP server '{}' initialize response parse error: {}",
                self.server_name, e,
            ))
        })?;

        *self.server_info.lock().await = result.server_info.clone();

        // Send initialized notification (no response expected)
        self.send_notification("notifications/initialized", None).await?;

        Ok(result)
    }
}
```

**Tool discovery and invocation:**

```rust
impl StdioTransport {
    /// Discover available tools from the MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDefinition>> {
        let response = self.send_request("tools/list", None).await?;

        let result: ToolsListResult = serde_json::from_value(
            response.result.ok_or_else(|| {
                ClosedCodeError::McpError(format!(
                    "MCP server '{}' tools/list returned error: {:?}",
                    self.server_name, response.error,
                ))
            })?,
        )
        .map_err(|e| {
            ClosedCodeError::McpError(format!(
                "MCP server '{}' tools/list parse error: {}",
                self.server_name, e,
            ))
        })?;

        Ok(result.tools)
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let params = ToolCallParams {
            name: tool_name.into(),
            arguments,
        };

        let response = tokio::time::timeout(
            Duration::from_secs(CALL_TIMEOUT_SECS),
            self.send_request("tools/call", Some(serde_json::to_value(&params).unwrap())),
        )
        .await
        .map_err(|_| {
            ClosedCodeError::McpError(format!(
                "MCP tool '{}/{}' call timed out after {}s",
                self.server_name, tool_name, CALL_TIMEOUT_SECS,
            ))
        })??;

        if let Some(err) = &response.error {
            return Err(ClosedCodeError::McpError(format!(
                "MCP tool '{}/{}' returned error: {} (code: {})",
                self.server_name, tool_name, err.message, err.code,
            )));
        }

        let result: ToolCallResult = serde_json::from_value(
            response.result.ok_or_else(|| {
                ClosedCodeError::McpError(format!(
                    "MCP tool '{}/{}' returned no result",
                    self.server_name, tool_name,
                ))
            })?,
        )
        .map_err(|e| {
            ClosedCodeError::McpError(format!(
                "MCP tool '{}/{}' result parse error: {}",
                self.server_name, tool_name, e,
            ))
        })?;

        Ok(result)
    }

    /// Gracefully shut down the MCP server.
    pub async fn shutdown(&self) -> Result<()> {
        // Try graceful shutdown
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.send_request("shutdown", None),
        )
        .await;

        // Send exit notification
        let _ = self.send_notification("exit", None).await;

        // Kill the process if still running
        let mut child = self.child.lock().await;
        let _ = child.kill().await;

        Ok(())
    }
}
```

**Message framing (private):**

```rust
impl StdioTransport {
    /// Send a JSON-RPC request and wait for the response.
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        let json = serde_json::to_string(&request).map_err(|e| {
            ClosedCodeError::McpError(format!("Failed to serialize JSON-RPC request: {}", e))
        })?;

        tracing::debug!("MCP [{}] → {}", self.server_name, json);

        // Write request + newline to stdin
        {
            let mut writer = self.writer.lock().await;
            writer.write_all(json.as_bytes()).await.map_err(|e| {
                ClosedCodeError::McpError(format!(
                    "Failed to write to MCP server '{}': {}",
                    self.server_name, e,
                ))
            })?;
            writer.write_all(b"\n").await.map_err(|e| {
                ClosedCodeError::McpError(format!(
                    "Failed to write newline to MCP server '{}': {}",
                    self.server_name, e,
                ))
            })?;
            writer.flush().await.map_err(|e| {
                ClosedCodeError::McpError(format!(
                    "Failed to flush MCP server '{}' stdin: {}",
                    self.server_name, e,
                ))
            })?;
        }

        // Read response (one JSON object per line)
        let response_line = {
            let mut reader = self.reader.lock().await;
            let mut line = String::new();
            reader.read_line(&mut line).await.map_err(|e| {
                ClosedCodeError::McpError(format!(
                    "Failed to read from MCP server '{}': {}",
                    self.server_name, e,
                ))
            })?;
            line
        };

        tracing::debug!("MCP [{}] ← {}", self.server_name, response_line.trim());

        let response: JsonRpcResponse =
            serde_json::from_str(response_line.trim()).map_err(|e| {
                ClosedCodeError::McpError(format!(
                    "Failed to parse MCP server '{}' response: {} (raw: {})",
                    self.server_name,
                    e,
                    response_line.trim(),
                ))
            })?;

        Ok(response)
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        let notification = JsonRpcNotification::new(method, params);
        let json = serde_json::to_string(&notification).map_err(|e| {
            ClosedCodeError::McpError(format!("Failed to serialize notification: {}", e))
        })?;

        tracing::debug!("MCP [{}] → {}", self.server_name, json);

        let mut writer = self.writer.lock().await;
        writer.write_all(json.as_bytes()).await.map_err(|e| {
            ClosedCodeError::McpError(format!(
                "Failed to write notification to MCP server '{}': {}",
                self.server_name, e,
            ))
        })?;
        writer.write_all(b"\n").await.ok();
        writer.flush().await.ok();

        Ok(())
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| Newline-delimited JSON framing | MCP STDIO transport uses one JSON object per line. Simplest framing, matches all MCP SDKs. |
| `Mutex` wrappers on child/stdin/stdout | Makes `StdioTransport` `Send + Sync`. MCP calls are sequential in practice, but mutexes allow safe sharing. |
| `AtomicI64` for request IDs | Lock-free, monotonically increasing. JSON-RPC IDs must be unique per connection. |
| 30s initialize timeout, 120s call timeout | Initialize should be fast. Tool calls may be slow (database queries, API calls). 120s matches explorer agent timeout. |
| `shutdown` attempts graceful then kills | Some MCP servers may not implement shutdown. `kill()` ensures cleanup. |
| `env` parameter on `spawn` | MCP servers often need environment variables (API keys, config). Passed from TOML config. |

---

## Sub-Phase 8b.7: McpToolProxy

### New File: `src/mcp/proxy.rs`

Wraps a single MCP server tool as a `Box<dyn Tool>` that can be registered in the `ToolRegistry`.

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::{FunctionDeclaration, Parameters};
use crate::mode::Mode;
use crate::tool::Tool;

use super::jsonrpc::{McpContent, McpToolDefinition};
use super::transport::StdioTransport;

/// A proxy tool that forwards calls to an MCP server.
///
/// Each McpToolProxy wraps a single tool from a single MCP server.
/// The tool name in the registry is `server_name/tool_name` to avoid
/// conflicts with built-in tools and tools from other servers.
#[derive(Debug)]
pub struct McpToolProxy {
    /// Namespaced tool name: "server_name/tool_name"
    namespaced_name: String,
    /// The original MCP tool name (used in tools/call request).
    mcp_tool_name: String,
    /// Tool description from the MCP server.
    description: String,
    /// JSON Schema for the tool's input parameters.
    input_schema: Value,
    /// Transport connection to the MCP server.
    transport: Arc<StdioTransport>,
}

impl McpToolProxy {
    /// Create a proxy for an MCP tool.
    pub fn new(
        server_name: &str,
        tool_def: &McpToolDefinition,
        transport: Arc<StdioTransport>,
    ) -> Self {
        Self {
            namespaced_name: format!("{}/{}", server_name, tool_def.name),
            mcp_tool_name: tool_def.name.clone(),
            description: tool_def.description.clone().unwrap_or_default(),
            input_schema: tool_def.input_schema.clone(),
            transport,
        }
    }

    /// Convert an MCP input_schema (JSON Schema) to Gemini Parameters.
    fn schema_to_parameters(schema: &Value) -> Parameters {
        let properties = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();

        let required = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        Parameters {
            schema_type: "object".into(),
            properties,
            required,
        }
    }

    /// Convert MCP content items to a JSON value for the tool result.
    fn content_to_json(content: &[McpContent]) -> Value {
        if content.len() == 1 {
            match &content[0] {
                McpContent::Text { text } => json!({ "result": text }),
                McpContent::Image { data, mime_type } => json!({
                    "result": format!("[Image: {} bytes, {}]", data.len(), mime_type),
                }),
                McpContent::Resource { resource } => json!({
                    "result": resource.text.as_deref().unwrap_or("[resource]"),
                    "uri": resource.uri,
                }),
            }
        } else {
            let items: Vec<Value> = content
                .iter()
                .map(|c| match c {
                    McpContent::Text { text } => json!({ "type": "text", "text": text }),
                    McpContent::Image { data, mime_type } => json!({
                        "type": "image", "size": data.len(), "mime_type": mime_type,
                    }),
                    McpContent::Resource { resource } => json!({
                        "type": "resource", "uri": resource.uri, "text": resource.text,
                    }),
                })
                .collect();
            json!({ "results": items })
        }
    }
}

#[async_trait]
impl Tool for McpToolProxy {
    fn name(&self) -> &str {
        &self.namespaced_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.namespaced_name.clone(),
            description: self.description.clone(),
            parameters: Self::schema_to_parameters(&self.input_schema),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        tracing::info!(
            "MCP tool call: {} (server: {})",
            self.mcp_tool_name,
            self.transport.server_name(),
        );

        let result = self.transport.call_tool(&self.mcp_tool_name, args).await?;

        if result.is_error == Some(true) {
            let error_text = result
                .content
                .iter()
                .filter_map(|c| match c {
                    McpContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            return Err(ClosedCodeError::McpError(format!(
                "MCP tool '{}' returned error: {}",
                self.namespaced_name, error_text,
            )));
        }

        Ok(Self::content_to_json(&result.content))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Explore, Mode::Plan, Mode::Guided, Mode::Execute, Mode::Auto]
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| `server_name/tool_name` namespacing | Prevents collisions across servers and with built-in tools. The LLM calls `my_server/get_data`. |
| `Arc<StdioTransport>` shared among proxy tools | All tools from the same server share one transport. Internal mutexes handle serialization. |
| `schema_to_parameters()` conversion | MCP uses JSON Schema; Gemini uses its own `Parameters`. The conversion extracts `properties` and `required`. |
| MCP tools available in all modes | MCP tools are external; the server controls access. Mode restriction would need per-tool config. |
| Error via `is_error` flag | MCP `tools/call` has an `isError` field. When true, content contains error text. |

---

## Sub-Phase 8b.8: Config + Startup Integration

### New File: `src/mcp/mod.rs`

Module root with re-exports and MCP lifecycle management.

```rust
pub mod jsonrpc;
pub mod proxy;
pub mod transport;

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::error::Result;
use crate::tool::registry::ToolRegistry;

use self::jsonrpc::McpToolDefinition;
use self::proxy::McpToolProxy;
use self::transport::StdioTransport;
```

**McpServerConfig:**

```rust
/// MCP server configuration from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    /// The command to run (e.g., "npx", "python", "/usr/local/bin/my-server").
    pub command: String,
    /// Command arguments.
    pub args: Option<Vec<String>>,
    /// Transport type. Currently only "stdio" is supported.
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Whether this server is enabled. Defaults to true.
    pub enabled: Option<bool>,
    /// Additional environment variables for the server process.
    pub env: Option<HashMap<String, String>>,
}

fn default_transport() -> String {
    "stdio".into()
}
```

**McpServer — connected server:**

```rust
/// A connected MCP server with its transport and discovered tools.
#[derive(Debug)]
pub struct McpServer {
    /// Server name (from config key).
    pub name: String,
    /// The STDIO transport connection.
    pub transport: Arc<StdioTransport>,
    /// Discovered tool definitions from this server.
    pub tools: Vec<McpToolDefinition>,
}
```

**Lifecycle functions:**

```rust
/// Initialize all configured MCP servers.
///
/// For each enabled server: spawn process → initialize handshake → discover tools.
/// Failed servers are logged and skipped — one failure doesn't block others.
pub async fn initialize_mcp_servers(
    configs: &HashMap<String, McpServerConfig>,
) -> Vec<McpServer> {
    let mut servers = Vec::new();

    for (name, config) in configs {
        if !config.enabled.unwrap_or(true) {
            tracing::info!("MCP server '{}' is disabled, skipping", name);
            continue;
        }

        tracing::info!(
            "Starting MCP server '{}': {} {:?}",
            name, config.command, config.args,
        );

        match start_mcp_server(name, config).await {
            Ok(server) => {
                tracing::info!(
                    "MCP server '{}' connected: {} tools discovered",
                    name, server.tools.len(),
                );
                servers.push(server);
            }
            Err(e) => {
                tracing::warn!("Failed to start MCP server '{}': {}", name, e);
                eprintln!("Warning: MCP server '{}' failed to start: {}", name, e);
            }
        }
    }

    servers
}

/// Start a single MCP server and perform the full handshake.
async fn start_mcp_server(name: &str, config: &McpServerConfig) -> Result<McpServer> {
    let transport = StdioTransport::spawn(
        name,
        &config.command,
        &config.args.clone().unwrap_or_default(),
        config.env.as_ref(),
    )
    .await?;

    let transport = Arc::new(transport);
    let _init_result = transport.initialize().await?;
    let tools = transport.list_tools().await?;

    Ok(McpServer {
        name: name.to_string(),
        transport,
        tools,
    })
}

/// Register all MCP server tools into a ToolRegistry.
pub fn register_mcp_tools(registry: &mut ToolRegistry, servers: &[McpServer]) {
    for server in servers {
        for tool_def in &server.tools {
            let proxy = McpToolProxy::new(&server.name, tool_def, server.transport.clone());
            registry.register(Box::new(proxy));
        }
    }
}

/// Shut down all connected MCP servers.
pub async fn shutdown_mcp_servers(servers: &[McpServer]) {
    for server in servers {
        tracing::info!("Shutting down MCP server '{}'", server.name);
        if let Err(e) = server.transport.shutdown().await {
            tracing::warn!("Error shutting down MCP server '{}': {}", server.name, e);
        }
    }
}
```

### `src/config.rs` — Modified

**Updated `TomlConfig`:**

```rust
use std::collections::HashMap;

#[derive(Debug, Default, Deserialize)]
pub struct TomlConfig {
    // ... existing fields ...
    #[serde(default)]
    pub mcp_servers: Option<HashMap<String, crate::mcp::McpServerConfig>>,
}
```

**Updated `Config` struct:**

```rust
#[derive(Debug, Clone)]
pub struct Config {
    // ... existing fields ...
    pub mcp_servers: HashMap<String, crate::mcp::McpServerConfig>,
}
```

**Updated `Config::from_cli()`:**

```rust
let mcp_servers = merged.mcp_servers.unwrap_or_default();
```

**Updated `Config::merge()`:**

```rust
fn merge(base: TomlConfig, overlay: TomlConfig) -> TomlConfig {
    TomlConfig {
        // ... existing merges ...
        mcp_servers: match (base.mcp_servers, overlay.mcp_servers) {
            (Some(mut base_map), Some(overlay_map)) => {
                base_map.extend(overlay_map);
                Some(base_map)
            }
            (base, overlay) => overlay.or(base),
        },
    }
}
```

**Example `config.toml`:**

```toml
model = "gemini-3.1-pro-preview"
default_mode = "execute"

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/project"]
transport = "stdio"

[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_..." }

[mcp_servers.disabled_server]
command = "some-server"
enabled = false
```

### `src/error.rs` — Modified

**New error variants:**

```rust
#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // ... existing variants ...

    // Image errors (Phase 8b)
    #[error("Image error: {0}")]
    ImageError(String),

    // MCP errors (Phase 8b)
    #[error("MCP error: {0}")]
    McpError(String),
}
```

### `src/lib.rs` — Modified

```rust
pub mod image;
pub mod mcp;
```

### `src/repl.rs` — Modified (MCP lifecycle + /mcp command)

**Updated `run_repl()` for MCP initialization/shutdown:**

```rust
pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    // ... existing client + sandbox + orchestrator setup ...

    // Initialize MCP servers
    let mcp_servers = crate::mcp::initialize_mcp_servers(&config.mcp_servers).await;
    let mcp_tool_count: usize = mcp_servers.iter().map(|s| s.tools.len()).sum();

    // Register MCP tools
    for server in &mcp_servers {
        crate::mcp::register_mcp_tools(&mut orchestrator.registry_mut(), std::slice::from_ref(server));
    }

    // Startup banner
    // ... existing banner ...
    if !mcp_servers.is_empty() {
        println!(
            "MCP: {} server(s), {} tool(s)",
            mcp_servers.len(), mcp_tool_count,
        );
    }

    // ... existing REPL loop ...

    // Shutdown MCP servers on exit
    crate::mcp::shutdown_mcp_servers(&mcp_servers).await;

    Ok(())
}
```

**New `/mcp` slash command:**

```rust
"/mcp" => {
    if mcp_servers.is_empty() {
        println!("No MCP servers configured.");
        println!("Add servers to config.toml under [mcp_servers.<name>]");
    } else {
        for server in &mcp_servers {
            println!(
                "  {} ({} tools)",
                server.name,
                server.tools.len(),
            );
            for tool in &server.tools {
                let desc = tool.description.as_deref().unwrap_or("");
                let desc_short = if desc.len() > 60 {
                    format!("{}...", &desc[..57])
                } else {
                    desc.to_string()
                };
                println!("    {}/{} — {}", server.name, tool.name, desc_short);
            }
        }
    }
    SlashResult::Continue
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| MCP servers initialized at REPL startup | Simple lifecycle. Servers are long-lived for the session. Dynamic connect/disconnect can be added later. |
| Failed servers logged and skipped | One failing server doesn't block the session. User is warned but can still work. |
| `mcp_servers` merged via `extend()` | Project config can override user config servers by name. New servers are added. |
| MCP tools registered into existing `ToolRegistry` | Uniform handling — the LLM doesn't know whether a tool is built-in or MCP. |
| `/mcp` is read-only | Lists connected servers and tools. No runtime connect/disconnect (deferred). |

---

## Test Summary

| File | New Tests | Category |
|------|-----------|----------|
| `src/image/mod.rs` | 12 | ImageMimeType from_extension (png, jpg, jpeg, gif, webp, unknown), from_magic_bytes (png, jpeg, gif, webp, short bytes), as_str, Display, read_image_file (success, not found, too large, unsupported format) |
| `src/agent/image_description.rs` | 4 | System prompt contains "image analyst", constants (timeout), new() creates instance, describe requires client |
| `src/tool/image.rs` | 6 | Tool properties (name, description, modes), declaration has path param, execute missing path, resolve_path relative/absolute |
| `src/mcp/jsonrpc.rs` | 14 | JsonRpcRequest serialization, JsonRpcResponse with result, JsonRpcResponse with error, JsonRpcNotification serialization, JsonRpcId number/string, InitializeParams serialization, ToolsListResult deserialization, McpToolDefinition deserialization, ToolCallParams serialization, ToolCallResult deserialization (text, image, error), McpContent tagged deserialization, error_codes constants |
| `src/mcp/transport.rs` | 4 | Spawn failure (bad command), server_name accessor, next_id increments, message framing format |
| `src/mcp/proxy.rs` | 8 | Namespaced name format, schema_to_parameters (empty, with properties, with required), content_to_json (single text, single image, multiple), description fallback, available_modes |
| `src/mcp/mod.rs` | 4 | McpServerConfig deserialization, default_transport, register_mcp_tools into registry, initialize with empty config |
| `src/config.rs` | 6 | mcp_servers TOML parsing, merge mcp_servers (overlay wins, extend), Config::from_cli with mcp_servers, default empty mcp_servers |
| `src/cli.rs` | 3 | Parse --image flag on ask, --image defaults to None, ask with both question and image |
| `src/error.rs` | 4 | ImageError display, McpError display, contain message text |
| `src/repl.rs` | 4 | /paste returns Continue, /mcp returns Continue, /help includes /paste and /mcp |
| `src/tool/registry.rs` | 3 | tool_names returns sorted list, create_orchestrator_registry includes read_image, MCP tools registered via register_mcp_tools |
| **Total** | **72 new tests** | |

---

## Milestone

```bash
# Image support — clipboard paste
cargo run -- --api-key $KEY
# closed-code
# Mode: explore | Model: gemini-3.1-pro-preview | Tools: 10
# Working directory: /Users/me/project
# Sandbox: workspace-write (macOS Seatbelt)

# Copy a screenshot to clipboard, then:
# explore > /paste What error is shown in this screenshot?
# Reading clipboard image...
# ✓ Image from clipboard (image/png, 145232 bytes)
# Analyzing image...
#
# ## Image Description
#
# The screenshot shows a terminal window with a Rust compiler error:
#
# ```
# error[E0308]: mismatched types
#   --> src/main.rs:42:5
#    |
# 42 |     let x: String = 42;
#    |                      ^^ expected `String`, found `i32`
# ```
#
# (Image description added to context)
#
# explore > How do I fix that error?
# The error is because you're assigning an integer to a String variable.
# Change line 42 to: `let x: String = "42".to_string();`

# Image support — LLM calls read_image tool
# explore > Read the screenshot at ./docs/architecture.png and explain the architecture
# ⠋ [tool] read_image(path: "docs/architecture.png")
# ✓ [tool] read_image(path: "docs/architecture.png")
#
# Based on the architecture diagram, the system has three layers...

# Image support — CLI one-shot
cargo run -- --api-key $KEY ask "What does this UI show?" --image ./screenshot.png
# Analyzing image...
# The UI shows a login form with two fields: email and password...

# MCP support — configured servers
# ~/.closed-code/config.toml:
# [mcp_servers.filesystem]
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/data"]
#
# [mcp_servers.github]
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-github"]
# env = { GITHUB_TOKEN = "ghp_abc123" }

cargo run -- --api-key $KEY
# closed-code
# Mode: explore | Model: gemini-3.1-pro-preview | Tools: 14
# Working directory: /Users/me/project
# Sandbox: workspace-write (macOS Seatbelt)
# MCP: 2 server(s), 4 tool(s)

# MCP tools are namespaced:
# explore > /mcp
#   filesystem (2 tools)
#     filesystem/read_file — Read a file from the filesystem
#     filesystem/list_directory — List directory contents
#   github (2 tools)
#     github/search_repos — Search GitHub repositories
#     github/get_issue — Get a GitHub issue by number

# The LLM calls MCP tools like built-in tools:
# explore > Search GitHub for Rust MCP libraries
# ⠋ [tool] github/search_repos(query: "rust mcp library")
# ✓ [tool] github/search_repos(query: "rust mcp library")
# Here are the top results for Rust MCP libraries:
# 1. mcp-rs — A Rust SDK for the Model Context Protocol...

# MCP server failure is graceful:
# Warning: MCP server 'broken' failed to start: Failed to spawn...
# (closed-code starts normally with remaining servers)

# Tests
cargo test
# running 457 tests (385 existing + 72 new)
# test image::tests::... ok
# test mcp::jsonrpc::tests::... ok
# test mcp::proxy::tests::... ok
# ...
# test result: ok. 457 passed; 0 failed
```

---

## Implementation Order

1. `Cargo.toml` — add `arboard = "3"`, `base64 = "0.22"`, `png = "0.17"` dependencies
2. `src/error.rs` — add `ImageError(String)`, `McpError(String)` variants
3. `src/image/mod.rs` — `ImageMimeType`, `ImageData`, `read_clipboard_image()`, `read_image_file()`, `encode_rgba_to_png()`
4. `src/lib.rs` — add `pub mod image;`
5. `cargo test` checkpoint — image utilities unit tests pass
6. `src/agent/image_description.rs` — `ImageDescriptionAgent` with `describe()` method
7. `src/agent/mod.rs` — add `pub mod image_description;`
8. `src/tool/image.rs` — `ReadImageTool` implementation
9. `src/tool/mod.rs` — add `pub mod image;`
10. `src/tool/registry.rs` — update `register_filesystem_tools()` with `client` param, register `ReadImageTool`, add `tool_names()` method
11. `cargo test` checkpoint — image tool + agent tests pass
12. `src/cli.rs` — add `--image` flag on `Commands::Ask`
13. `src/agent/orchestrator.rs` — add `inject_image_context()`, `client()` accessor
14. `src/repl.rs` — add `/paste` slash command, update `run_oneshot()` for `--image`, update `/help`
15. `src/main.rs` — pass `image` to `run_oneshot()`
16. `cargo test` checkpoint — /paste + --image integration tests pass
17. `src/mcp/jsonrpc.rs` — all JSON-RPC 2.0 types and MCP message types
18. `src/mcp/transport.rs` — `StdioTransport` (spawn, initialize, list_tools, call_tool, shutdown)
19. `src/mcp/proxy.rs` — `McpToolProxy` wrapping MCP tools as `Box<dyn Tool>`
20. `src/mcp/mod.rs` — `McpServer`, `McpServerConfig`, lifecycle functions
21. `src/lib.rs` — add `pub mod mcp;`
22. `src/config.rs` — add `mcp_servers` to `TomlConfig` and `Config`, update `merge()` and `from_cli()`
23. `src/repl.rs` — add `/mcp` slash command, MCP initialization/shutdown in `run_repl()`, update `/status`
24. `cargo test` — all 457 tests pass (385 existing + 72 new)

---

## Complexity: **High**

Two independent subsystems (Image + MCP) that both integrate into the tool registry and orchestrator. Image support requires cross-platform clipboard access (`arboard`), PNG encoding, a sub-agent with multimodal API calls (`Part::InlineData`), and integration at three surfaces (tool, slash command, CLI flag). MCP support requires JSON-RPC 2.0 message framing, child process lifecycle management, async stdin/stdout communication, dynamic tool proxy wrapping, and config/startup integration. ~7 new files, ~10 modified files, ~72 new tests, ~2,500 lines.

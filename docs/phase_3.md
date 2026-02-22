# Phase 3: Sub-Agent Architecture

**Goal**: The orchestrator can spawn Explorer, Planner, and Web Search sub-agents as Gemini function calls. Sub-agents independently research and report back. The REPL's direct API call is replaced with an orchestrator that manages conversation history, mode-specific tool sets, and context window pruning.

**Deliverable**: `cargo run` launches the REPL backed by an orchestrator that can delegate research tasks to sub-agents. In Explore mode, the orchestrator can spawn an ExplorerAgent. In Plan mode, it can additionally spawn a PlannerAgent and WebSearchAgent. Sub-agents run their own non-streaming tool loops, produce structured reports, and return results to the orchestrator's conversation.

**Builds on**: Phase 2 (Tool trait, ToolRegistry, filesystem tools, ShellCommandTool, `run_tool_loop`, `StreamResult`, `FunctionDeclaration`, `ToolDefinition`, `ToolConfig`, `FunctionCallingConfig`, `Content::function_responses()`, `GenerateContentResponse::function_calls()` / `has_function_calls()`, all Phase 2 error variants).

**Estimated**: ~2,500 lines of new Rust across ~8 new files + modifications to ~5 existing Phase 2 files.

---

## File Layout

### New Files

```
src/
  agent/
    mod.rs             # Agent trait, module re-exports
    message.rs         # AgentRequest, AgentResponse, Artifact, ArtifactType
    explorer.rs        # ExplorerAgent (read-only codebase research)
    planner.rs         # PlannerAgent (structured plan creation)
    web_searcher.rs    # WebSearchAgent (Gemini + google_search grounding)
    orchestrator.rs    # Orchestrator: conversation history, mode mgmt, sub-agent dispatch
  tool/
    spawn.rs           # SpawnExplorerTool, SpawnPlannerTool, SpawnWebSearchTool
    report.rs          # CreateReportTool (sub-agents structure output)
```

### Modified Phase 2 Files

```
src/
  lib.rs               # + pub mod agent;
  error.rs             # + Agent-related error variants
  gemini/
    types.rs           # + GeminiTool enum, GoogleSearchTool, GroundingMetadata types
                       # + Change GenerateContentRequest.tools type
  tool/
    mod.rs             # + pub mod spawn; pub mod report;
    registry.rs        # + create_orchestrator_registry(), create_subagent_registry()
  repl.rs              # Replace direct API call with Orchestrator::handle_user_input()
```

---

## Dependencies to Add

No new crate dependencies are needed for Phase 3. All required crates are already present from Phase 1/2:

| Existing Crate | Phase 3 Usage |
|----------------|---------------|
| `async-trait` | Agent trait definition |
| `serde` / `serde_json` | AgentRequest/AgentResponse serialization, google_search grounding types |
| `uuid` | AgentRequest IDs |
| `tokio` | Sub-agent timeout (`tokio::time::timeout`), `Arc` for shared client |
| `tracing` | Sub-agent activity logging |

**Why no new dependencies**: Sub-agents reuse `GeminiClient`, `ToolRegistry`, `run_tool_loop`, and all Phase 2 types. The google_search grounding feature is a native Gemini API capability, not a third-party tool.

---

## Gemini API Reference: Google Search Grounding

This section documents the Gemini google_search grounding protocol that the WebSearchAgent uses. This is distinct from function calling — the model automatically searches, processes, and cites information.

### Request Format

```json
{
  "contents": [
    {
      "role": "user",
      "parts": [{ "text": "What are the latest Rust async patterns?" }]
    }
  ],
  "systemInstruction": {
    "parts": [{ "text": "You are a research assistant..." }]
  },
  "tools": [
    { "google_search": {} }
  ],
  "generationConfig": {
    "temperature": 0.7,
    "maxOutputTokens": 4096
  }
}
```

**Key points**:
- The `tools` array contains `{"google_search": {}}` instead of `{"functionDeclarations": [...]}`.
- There is **no tool-call loop** — the model handles searching internally.
- The response is a single text response with grounding metadata.
- `google_search` and `functionDeclarations` **can coexist** in the same `tools` array, but the WebSearchAgent uses google_search exclusively.
- No `toolConfig` is needed for google_search (it is only for `functionCallingConfig`).

### Response with Grounding Metadata

```json
{
  "candidates": [{
    "content": {
      "role": "model",
      "parts": [{ "text": "Based on recent developments..." }]
    },
    "finishReason": "STOP",
    "groundingMetadata": {
      "webSearchQueries": ["Rust async patterns 2026"],
      "groundingChunks": [
        {
          "web": {
            "uri": "https://example.com/article",
            "title": "Modern Rust Async"
          }
        }
      ],
      "groundingSupports": [
        {
          "segment": { "startIndex": 0, "endIndex": 45 },
          "groundingChunkIndices": [0]
        }
      ]
    }
  }],
  "usageMetadata": { ... }
}
```

**Key points**:
- `groundingMetadata` appears on the candidate, alongside `content` and `finishReason`.
- `webSearchQueries` shows what the model searched for.
- `groundingChunks` contains the sources (URLs and titles).
- `groundingSupports` maps text segments to source indices for attribution.

---

## Phase 2 Modifications

### `src/lib.rs` — Add Agent Module

```rust
pub mod cli;
pub mod config;
pub mod error;
pub mod gemini;
pub mod mode;
pub mod repl;
pub mod ui;
pub mod tool;   // Phase 2
pub mod agent;  // Phase 3

pub use config::Config;
pub use error::ClosedCodeError;
pub use mode::Mode;
```

### `src/error.rs` — New Agent Variants

Add these after the existing Phase 2 tool error variants:

```rust
#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // ... existing Phase 1 + Phase 2 variants ...

    // Agent errors (Phase 3)
    #[error("Agent '{agent_id}' failed: {message}")]
    AgentError { agent_id: String, message: String },

    #[error("Agent '{agent_id}' timed out after {seconds}s")]
    AgentTimeout { agent_id: String, seconds: u64 },

    #[error("Orchestrator exceeded max iterations ({max}) for this turn")]
    OrchestratorMaxIterations { max: usize },

    #[error("Sub-agent tool loop exceeded max iterations ({max}) for agent '{agent_id}'")]
    SubAgentMaxIterations { agent_id: String, max: usize },
}
```

### `src/gemini/types.rs` — Google Search and GeminiTool Types

**New polymorphic tool type** — Replace `ToolDefinition` in the `tools` field with a `GeminiTool` enum that supports both function declarations and google_search grounding:

```rust
/// Represents a tool entry in the Gemini API `tools` array.
/// Can be either function declarations or google_search grounding.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum GeminiTool {
    /// Function calling tool with declarations.
    Functions(ToolDefinition),
    /// Google Search grounding tool.
    GoogleSearch(GoogleSearchTool),
}

#[derive(Debug, Clone, Serialize)]
pub struct GoogleSearchTool {
    pub google_search: GoogleSearchConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoogleSearchConfig {}  // Empty object: {"google_search": {}}

impl GoogleSearchTool {
    pub fn new() -> Self {
        Self {
            google_search: GoogleSearchConfig {},
        }
    }
}
```

**Update `GenerateContentRequest`** — Change the `tools` field type:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiTool>>,       // Changed from Vec<ToolDefinition>
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,
}
```

**Update `ToolRegistry::to_gemini_tools()`** — Return `GeminiTool::Functions` wrapper:

```rust
pub fn to_gemini_tools(&self, mode: &Mode) -> Option<Vec<GeminiTool>> {
    let declarations = self.declarations_for_mode(mode);
    if declarations.is_empty() {
        None
    } else {
        Some(vec![GeminiTool::Functions(ToolDefinition {
            function_declarations: declarations,
        })])
    }
}
```

**New grounding metadata types** for deserializing google_search responses:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingMetadata {
    #[serde(default)]
    pub web_search_queries: Vec<String>,
    #[serde(default)]
    pub grounding_chunks: Vec<GroundingChunk>,
    #[serde(default)]
    pub grounding_supports: Vec<GroundingSupport>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroundingChunk {
    pub web: Option<WebSource>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSource {
    pub uri: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingSupport {
    pub segment: Option<GroundingSegment>,
    #[serde(default)]
    pub grounding_chunk_indices: Vec<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingSegment {
    pub start_index: Option<usize>,
    pub end_index: Option<usize>,
}
```

**Extend `Candidate`** with grounding metadata:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub content: Option<Content>,
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub safety_ratings: Vec<SafetyRating>,
    // Phase 3 addition:
    pub grounding_metadata: Option<GroundingMetadata>,
}
```

### `src/tool/mod.rs` — Add New Modules

```rust
pub mod registry;
pub mod filesystem;
pub mod shell;
pub mod spawn;   // Phase 3
pub mod report;  // Phase 3
```

### `src/tool/registry.rs` — New Registry Factory Functions

Add specialized registry constructors for different contexts:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use crate::gemini::GeminiClient;
use crate::mode::Mode;

/// Create a ToolRegistry for the orchestrator in a given mode.
/// Includes filesystem tools + spawn tools based on mode.
pub fn create_orchestrator_registry(
    working_directory: PathBuf,
    mode: &Mode,
    client: Arc<GeminiClient>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    // Filesystem tools (all modes)
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
        super::shell::ShellCommandTool::new(working_directory.clone())
    ));

    // Spawn tools (mode-dependent)
    match mode {
        Mode::Explore => {
            registry.register(Box::new(
                super::spawn::SpawnExplorerTool::new(
                    client.clone(), working_directory.clone()
                )
            ));
        }
        Mode::Plan => {
            registry.register(Box::new(
                super::spawn::SpawnExplorerTool::new(
                    client.clone(), working_directory.clone()
                )
            ));
            registry.register(Box::new(
                super::spawn::SpawnPlannerTool::new(
                    client.clone(), working_directory.clone()
                )
            ));
            registry.register(Box::new(
                super::spawn::SpawnWebSearchTool::new(
                    client.clone(), working_directory.clone()
                )
            ));
        }
        Mode::Execute => {
            registry.register(Box::new(
                super::spawn::SpawnExplorerTool::new(
                    client.clone(), working_directory.clone()
                )
            ));
            // Write tools added in Phase 4
        }
    }

    registry
}

/// Create a ToolRegistry for sub-agents (Explorer/Planner).
/// Read-only tools + create_report. No spawn tools (sub-agents cannot spawn sub-agents).
pub fn create_subagent_registry(working_directory: PathBuf) -> ToolRegistry {
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
        super::shell::ShellCommandTool::new(working_directory.clone())
    ));
    registry.register(Box::new(
        super::report::CreateReportTool::new()
    ));

    registry
}
```

### `src/repl.rs` — Replace Direct API Call with Orchestrator

The REPL's main loop changes to delegate all user input processing to the Orchestrator:

```rust
use std::sync::Arc;
use crate::agent::orchestrator::Orchestrator;
use crate::gemini::GeminiClient;

pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(config.api_key.clone(), config.model.clone()));
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
    );
    let mut editor = DefaultEditor::new()?;

    println!("{}", styled_text("closed-code", Theme::ACCENT));
    println!("Mode: {} | Model: {} | Tools: {}",
        config.mode, config.model, orchestrator.tool_count());
    println!("Type /help for commands, /quit to exit.\n");

    loop {
        let prompt = format!("{} > ", orchestrator.mode());
        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() { continue; }
                let _ = editor.add_history_entry(line);

                if line.starts_with('/') {
                    match handle_slash_command(line, &mut orchestrator) {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                    }
                    continue;
                }

                // Delegate to orchestrator
                let spinner = Spinner::new("Thinking...");

                match orchestrator.handle_user_input(
                    line,
                    config,
                    |text| {
                        print!("{}", text);
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    },
                    |msg| {
                        spinner.set_message(msg);
                    },
                ).await {
                    Ok(_) => {
                        spinner.finish();
                        println!();
                    }
                    Err(e) => {
                        spinner.finish();
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

/// Update slash command handler to work with Orchestrator instead of raw history.
fn handle_slash_command(input: &str, orchestrator: &mut Orchestrator) -> SlashResult {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();

    match command.as_str() {
        "/quit" | "/exit" | "/q" => SlashResult::Quit,
        "/clear" => {
            orchestrator.clear_history();
            println!("History cleared.");
            SlashResult::Continue
        }
        "/help" => {
            println!("Available commands:");
            println!("  /help    — Show this message");
            println!("  /clear   — Clear conversation history");
            println!("  /quit    — Exit the REPL");
            SlashResult::Continue
        }
        _ => {
            println!("Unknown command: {}. Type /help for available commands.", command);
            SlashResult::Continue
        }
    }
}

/// Update run_oneshot to use Orchestrator too.
pub async fn run_oneshot(config: &Config, question: &str) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(config.api_key.clone(), config.model.clone()));
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
    );

    let spinner = Spinner::new("Thinking...");

    match orchestrator.handle_user_input(
        question,
        config,
        |text| {
            print!("{}", text);
            use std::io::Write;
            std::io::stdout().flush().ok();
        },
        |msg| {
            spinner.set_message(msg);
        },
    ).await {
        Ok(_) => {
            spinner.finish();
            println!();
        }
        Err(e) => {
            spinner.finish();
            eprintln!("Error: {}", e);
        }
    }

    Ok(())
}
```

---

## Implementation Details

### `src/agent/mod.rs`

The Agent trait defines the interface for all sub-agents. It is intentionally different from the Tool trait — agents have their own conversation state, system prompts, and tool registries.

```rust
use async_trait::async_trait;
use std::fmt::Debug;

use crate::error::Result;
use crate::gemini::GeminiClient;

pub mod message;
pub mod explorer;
pub mod planner;
pub mod web_searcher;
pub mod orchestrator;

pub use message::{AgentRequest, AgentResponse};

/// A sub-agent that can independently research and report back.
///
/// Key differences from the Tool trait:
/// - Agents own their own ToolRegistry and run their own tool-call loops.
/// - Agents have system prompts and maintain internal conversation state.
/// - Agents produce structured AgentResponse (not raw JSON Value).
/// - Agents are long-running (multiple API calls), not single-shot.
#[async_trait]
pub trait Agent: Send + Sync + Debug {
    /// Unique identifier for this agent type (e.g., "explorer", "planner").
    fn agent_type(&self) -> &str;

    /// The system prompt that guides this agent's behavior.
    fn system_prompt(&self) -> &str;

    /// Maximum tool-call iterations before forced completion.
    fn max_iterations(&self) -> usize;

    /// Run the agent with the given request.
    /// The agent creates its own conversation, runs its tool loop,
    /// and returns a structured response.
    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse>;
}
```

**Agent vs Tool trait comparison**:

| Aspect | Tool trait | Agent trait |
|--------|-----------|-------------|
| Execution model | Single function call | Multiple API calls with tool loop |
| State | Stateless | Own conversation history |
| Registry | Registered in a registry | Owns its own registry |
| Output | `Value` (raw JSON) | `AgentResponse` (structured) |
| Duration | Milliseconds | Seconds to minutes |
| Streaming | N/A | Non-streaming (internal) |

### `src/agent/message.rs`

Request and response types for agent communication.

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Request sent to a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Unique ID for tracking this request.
    pub id: Uuid,
    /// The task description (what the agent should research/plan/search for).
    pub task: String,
    /// Optional context strings from the orchestrator's conversation.
    /// Provides relevant background without sending the entire history.
    pub context: Vec<String>,
    /// Working directory for filesystem operations.
    pub working_directory: String,
}

impl AgentRequest {
    pub fn new(task: String, working_directory: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            task,
            context: Vec::new(),
            working_directory,
        }
    }

    pub fn with_context(mut self, context: Vec<String>) -> Self {
        self.context = context;
        self
    }
}

/// Structured response from a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The request ID this response corresponds to.
    pub request_id: Uuid,
    /// Which agent produced this response.
    pub agent_type: String,
    /// A brief summary suitable for the orchestrator to reference.
    /// Typically 1-3 sentences.
    pub summary: String,
    /// The detailed findings/plan/research results.
    /// Can be multiple paragraphs with markdown formatting.
    pub detailed_report: String,
    /// Structured artifacts (code snippets, file listings, etc.)
    pub artifacts: Vec<Artifact>,
}

/// A structured piece of output from a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Human-readable name for this artifact.
    pub name: String,
    /// Type of artifact.
    pub artifact_type: ArtifactType,
    /// The artifact's content.
    pub content: String,
}

/// Types of artifacts that sub-agents can produce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    /// A code snippet with language identifier.
    CodeSnippet { language: String },
    /// Contents of a specific file.
    FileContent { path: String },
    /// A directory listing.
    DirectoryListing,
    /// Search results (grep/file search).
    SearchResults,
    /// An implementation plan with steps.
    Plan,
    /// A diff (future use).
    Diff,
    /// Web search results with sources.
    WebSearchResults { sources: Vec<WebSource> },
}

/// A web source from grounded search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSource {
    pub url: String,
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_request_new() {
        let req = AgentRequest::new(
            "Analyze the error handling".into(),
            "/tmp/project".into(),
        );
        assert_eq!(req.task, "Analyze the error handling");
        assert_eq!(req.working_directory, "/tmp/project");
        assert!(req.context.is_empty());
    }

    #[test]
    fn agent_request_with_context() {
        let req = AgentRequest::new("task".into(), "/tmp".into())
            .with_context(vec!["The user asked about error handling".into()]);
        assert_eq!(req.context.len(), 1);
    }

    #[test]
    fn agent_response_serialization_roundtrip() {
        let response = AgentResponse {
            request_id: Uuid::nil(),
            agent_type: "explorer".into(),
            summary: "Found 3 error types".into(),
            detailed_report: "Detailed analysis...".into(),
            artifacts: vec![Artifact {
                name: "error.rs".into(),
                artifact_type: ArtifactType::CodeSnippet {
                    language: "rust".into(),
                },
                content: "pub enum Error { ... }".into(),
            }],
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: AgentResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.summary, "Found 3 error types");
        assert_eq!(parsed.artifacts.len(), 1);
    }

    #[test]
    fn artifact_types() {
        let code = ArtifactType::CodeSnippet { language: "rust".into() };
        assert!(matches!(code, ArtifactType::CodeSnippet { .. }));

        let web = ArtifactType::WebSearchResults {
            sources: vec![WebSource {
                url: "https://example.com".into(),
                title: Some("Example".into()),
            }],
        };
        assert!(matches!(web, ArtifactType::WebSearchResults { .. }));
    }
}
```

### `src/agent/explorer.rs`

The ExplorerAgent researches a codebase to answer questions. It runs its own tool-call loop with read-only tools plus the `create_report` tool.

```rust
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

use crate::agent::{Agent, AgentRequest, AgentResponse};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::GeminiClient;
use crate::gemini::types::*;
use crate::tool::registry::create_subagent_registry;

const EXPLORER_MAX_ITERATIONS: usize = 15;
const EXPLORER_TIMEOUT_SECS: u64 = 120;

const EXPLORER_SYSTEM_PROMPT: &str = "\
You are an expert code explorer agent. Your job is to thoroughly research a codebase \
to answer questions about its architecture, patterns, and implementation details.

You have access to filesystem tools (read_file, list_directory, search_files, grep, shell) \
and a create_report tool. Your workflow:

1. Start by understanding the project structure (list_directory, search_files).
2. Read relevant files to understand the code.
3. Use grep to find specific patterns, usages, or references.
4. Use shell for git log, cargo commands, or other read-only operations.
5. When you have gathered enough information, call create_report with your findings.

IMPORTANT: You MUST call create_report when you are done. This is how your findings \
are communicated back. Do not simply respond with text — always use create_report.

Be thorough but efficient. Focus on answering the specific task, not exploring everything. \
Include relevant code snippets in your report as artifacts.";

#[derive(Debug)]
pub struct ExplorerAgent {
    working_directory: PathBuf,
}

impl ExplorerAgent {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }

    /// Run the sub-agent's tool-call loop.
    /// Returns when create_report is called or max iterations reached.
    async fn run_subagent_loop(
        &self,
        client: &GeminiClient,
        history: &mut Vec<Content>,
        system_instruction: Content,
        tools: Option<Vec<GeminiTool>>,
        tool_config: Option<ToolConfig>,
    ) -> Result<Option<AgentResponse>> {
        let registry = create_subagent_registry(self.working_directory.clone());

        for iteration in 0..self.max_iterations() {
            tracing::debug!(
                "Explorer agent loop iteration {}/{}",
                iteration + 1,
                self.max_iterations()
            );

            let request = GenerateContentRequest {
                contents: history.clone(),
                system_instruction: Some(system_instruction.clone()),
                generation_config: Some(GenerationConfig {
                    temperature: Some(0.7),
                    top_p: None,
                    top_k: None,
                    max_output_tokens: Some(8192),
                }),
                tools: tools.clone(),
                tool_config: tool_config.clone(),
            };

            let response = client.generate_content(&request).await?;

            let candidate = response.candidates.first()
                .ok_or(ClosedCodeError::EmptyResponse)?;
            let content = candidate.content.as_ref()
                .ok_or(ClosedCodeError::EmptyResponse)?;

            // Separate text and function calls
            let mut function_calls = Vec::new();
            for part in &content.parts {
                if let Part::FunctionCall { name, args } = part {
                    function_calls.push((name.clone(), args.clone()));
                }
            }

            // If no function calls, the agent is done (no create_report — fallback)
            if function_calls.is_empty() {
                history.push(content.clone());

                let text = content.parts.iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");

                if !text.is_empty() {
                    return Ok(Some(AgentResponse {
                        request_id: uuid::Uuid::nil(), // Caller sets this
                        agent_type: self.agent_type().into(),
                        summary: text.chars().take(200).collect(),
                        detailed_report: text,
                        artifacts: Vec::new(),
                    }));
                }
                break;
            }

            // Append model's response to history
            history.push(content.clone());

            // Execute function calls
            let mut response_parts = Vec::new();
            for (name, args) in &function_calls {
                // Check if this is a create_report call — intercept and extract
                if name == "create_report" {
                    let report = Self::extract_report(args)?;
                    return Ok(Some(report));
                }

                let result = match registry.execute(name, args.clone()).await {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!("Explorer tool '{}' failed: {}", name, e);
                        serde_json::json!({"error": e.to_string()})
                    }
                };

                response_parts.push(Part::FunctionResponse {
                    name: name.clone(),
                    response: result,
                });
            }

            history.push(Content::function_responses(response_parts));
        }

        // Max iterations reached without create_report
        tracing::warn!("Explorer agent exhausted {} iterations", self.max_iterations());
        Ok(None)
    }

    /// Extract an AgentResponse from create_report tool arguments.
    fn extract_report(args: &serde_json::Value) -> Result<AgentResponse> {
        let summary = args["summary"].as_str()
            .unwrap_or("No summary provided")
            .to_string();

        let detailed_report = args["detailed_report"].as_str()
            .unwrap_or("No detailed report provided")
            .to_string();

        let artifacts = if let Some(snippets) = args["code_snippets"].as_array() {
            snippets.iter().filter_map(|s| {
                let name = s["name"].as_str()?.to_string();
                let language = s["language"].as_str()
                    .unwrap_or("text").to_string();
                let content = s["content"].as_str()?.to_string();
                Some(crate::agent::message::Artifact {
                    name,
                    artifact_type: crate::agent::message::ArtifactType::CodeSnippet {
                        language,
                    },
                    content,
                })
            }).collect()
        } else {
            Vec::new()
        };

        Ok(AgentResponse {
            request_id: uuid::Uuid::nil(), // Caller sets this
            agent_type: "explorer".into(),
            summary,
            detailed_report,
            artifacts,
        })
    }
}

#[async_trait]
impl Agent for ExplorerAgent {
    fn agent_type(&self) -> &str { "explorer" }

    fn system_prompt(&self) -> &str { EXPLORER_SYSTEM_PROMPT }

    fn max_iterations(&self) -> usize { EXPLORER_MAX_ITERATIONS }

    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let registry = create_subagent_registry(self.working_directory.clone());
        let tools = registry.to_gemini_tools(&crate::mode::Mode::Explore);
        let tool_config = Some(crate::tool::registry::ToolRegistry::tool_config());
        let system_instruction = Content::system(self.system_prompt());

        // Build initial message from the request
        let mut user_message = format!("Task: {}\n", request.task);
        if !request.context.is_empty() {
            user_message.push_str("\nContext from the conversation:\n");
            for ctx in &request.context {
                user_message.push_str(&format!("- {}\n", ctx));
            }
        }
        user_message.push_str(&format!(
            "\nWorking directory: {}",
            request.working_directory
        ));

        let mut history = vec![Content::user(&user_message)];

        // Run with timeout
        let result = tokio::time::timeout(
            Duration::from_secs(EXPLORER_TIMEOUT_SECS),
            self.run_subagent_loop(
                client,
                &mut history,
                system_instruction,
                tools,
                tool_config,
            ),
        )
        .await
        .map_err(|_| ClosedCodeError::AgentTimeout {
            agent_id: self.agent_type().into(),
            seconds: EXPLORER_TIMEOUT_SECS,
        })??;

        match result {
            Some(mut response) => {
                response.request_id = request.id;
                Ok(response)
            }
            None => {
                // Agent finished without a report — produce a minimal one
                Ok(AgentResponse {
                    request_id: request.id,
                    agent_type: self.agent_type().into(),
                    summary: "Explorer completed but did not produce a structured report.".into(),
                    detailed_report: "The explorer agent exhausted its iterations without \
                        calling create_report. The research may be incomplete.".into(),
                    artifacts: Vec::new(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_report_basic() {
        let args = serde_json::json!({
            "summary": "Found the main entry point",
            "detailed_report": "The project uses tokio as its async runtime...",
        });
        let report = ExplorerAgent::extract_report(&args).unwrap();
        assert_eq!(report.summary, "Found the main entry point");
        assert!(report.artifacts.is_empty());
    }

    #[test]
    fn extract_report_with_snippets() {
        let args = serde_json::json!({
            "summary": "Found error types",
            "detailed_report": "The error handling uses thiserror...",
            "code_snippets": [
                {
                    "name": "error.rs",
                    "language": "rust",
                    "content": "pub enum ClosedCodeError { ... }"
                }
            ]
        });
        let report = ExplorerAgent::extract_report(&args).unwrap();
        assert_eq!(report.artifacts.len(), 1);
        assert_eq!(report.artifacts[0].name, "error.rs");
    }

    #[test]
    fn extract_report_missing_fields() {
        let args = serde_json::json!({});
        let report = ExplorerAgent::extract_report(&args).unwrap();
        assert_eq!(report.summary, "No summary provided");
        assert_eq!(report.detailed_report, "No detailed report provided");
    }

    #[test]
    fn explorer_agent_properties() {
        let agent = ExplorerAgent::new(PathBuf::from("/tmp"));
        assert_eq!(agent.agent_type(), "explorer");
        assert_eq!(agent.max_iterations(), 15);
        assert!(agent.system_prompt().contains("code explorer"));
    }
}
```

### `src/agent/planner.rs`

The PlannerAgent creates structured implementation plans. It shares the same architecture as the ExplorerAgent but with a different system prompt and output format.

```rust
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

use crate::agent::{Agent, AgentRequest, AgentResponse};
use crate::agent::message::{Artifact, ArtifactType};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::GeminiClient;
use crate::gemini::types::*;
use crate::tool::registry::create_subagent_registry;

const PLANNER_MAX_ITERATIONS: usize = 15;
const PLANNER_TIMEOUT_SECS: u64 = 120;

const PLANNER_SYSTEM_PROMPT: &str = "\
You are an expert software architect and planning agent. Your job is to analyze a codebase \
and create detailed, actionable implementation plans.

You have access to filesystem tools (read_file, list_directory, search_files, grep, shell) \
and a create_report tool. Your workflow:

1. Understand the current codebase structure and patterns.
2. Read relevant existing code to understand conventions.
3. Identify dependencies, affected files, and potential challenges.
4. Create a structured plan with clear steps.
5. Call create_report with your plan.

Your plan should include:
- Step-by-step implementation order (numbered)
- Files to create or modify (with rationale)
- Key code patterns to follow (from existing codebase)
- Potential risks or trade-offs
- Estimated complexity per step

IMPORTANT: You MUST call create_report when done. The summary should be a brief overview \
of the plan. The detailed_report should contain the full plan. Include code snippets \
showing proposed implementations or patterns to follow.";

#[derive(Debug)]
pub struct PlannerAgent {
    working_directory: PathBuf,
}

impl PlannerAgent {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }

    /// Reuses the same sub-agent loop pattern as ExplorerAgent.
    /// The only differences are the system prompt and how the report
    /// is interpreted (Plan artifact type instead of CodeSnippet).
    async fn run_subagent_loop(
        &self,
        client: &GeminiClient,
        history: &mut Vec<Content>,
        system_instruction: Content,
        tools: Option<Vec<GeminiTool>>,
        tool_config: Option<ToolConfig>,
    ) -> Result<Option<AgentResponse>> {
        let registry = create_subagent_registry(self.working_directory.clone());

        for iteration in 0..self.max_iterations() {
            tracing::debug!(
                "Planner agent loop iteration {}/{}",
                iteration + 1,
                self.max_iterations()
            );

            let request = GenerateContentRequest {
                contents: history.clone(),
                system_instruction: Some(system_instruction.clone()),
                generation_config: Some(GenerationConfig {
                    temperature: Some(0.7),
                    top_p: None,
                    top_k: None,
                    max_output_tokens: Some(8192),
                }),
                tools: tools.clone(),
                tool_config: tool_config.clone(),
            };

            let response = client.generate_content(&request).await?;

            let candidate = response.candidates.first()
                .ok_or(ClosedCodeError::EmptyResponse)?;
            let content = candidate.content.as_ref()
                .ok_or(ClosedCodeError::EmptyResponse)?;

            let mut function_calls = Vec::new();
            for part in &content.parts {
                if let Part::FunctionCall { name, args } = part {
                    function_calls.push((name.clone(), args.clone()));
                }
            }

            if function_calls.is_empty() {
                history.push(content.clone());
                let text = content.parts.iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");

                if !text.is_empty() {
                    return Ok(Some(AgentResponse {
                        request_id: uuid::Uuid::nil(),
                        agent_type: self.agent_type().into(),
                        summary: text.chars().take(200).collect(),
                        detailed_report: text.clone(),
                        artifacts: vec![Artifact {
                            name: "Implementation Plan".into(),
                            artifact_type: ArtifactType::Plan,
                            content: text,
                        }],
                    }));
                }
                break;
            }

            history.push(content.clone());

            let mut response_parts = Vec::new();
            for (name, args) in &function_calls {
                if name == "create_report" {
                    let report = Self::extract_report(args)?;
                    return Ok(Some(report));
                }

                let result = match registry.execute(name, args.clone()).await {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!("Planner tool '{}' failed: {}", name, e);
                        serde_json::json!({"error": e.to_string()})
                    }
                };

                response_parts.push(Part::FunctionResponse {
                    name: name.clone(),
                    response: result,
                });
            }

            history.push(Content::function_responses(response_parts));
        }

        Ok(None)
    }

    fn extract_report(args: &serde_json::Value) -> Result<AgentResponse> {
        let summary = args["summary"].as_str()
            .unwrap_or("No summary provided")
            .to_string();

        let detailed_report = args["detailed_report"].as_str()
            .unwrap_or("No detailed report provided")
            .to_string();

        let mut artifacts = vec![Artifact {
            name: "Implementation Plan".into(),
            artifact_type: ArtifactType::Plan,
            content: detailed_report.clone(),
        }];

        // Also extract code snippets if provided
        if let Some(snippets) = args["code_snippets"].as_array() {
            for s in snippets {
                if let (Some(name), Some(content)) =
                    (s["name"].as_str(), s["content"].as_str())
                {
                    artifacts.push(Artifact {
                        name: name.to_string(),
                        artifact_type: ArtifactType::CodeSnippet {
                            language: s["language"].as_str()
                                .unwrap_or("text").to_string(),
                        },
                        content: content.to_string(),
                    });
                }
            }
        }

        Ok(AgentResponse {
            request_id: uuid::Uuid::nil(),
            agent_type: "planner".into(),
            summary,
            detailed_report,
            artifacts,
        })
    }
}

#[async_trait]
impl Agent for PlannerAgent {
    fn agent_type(&self) -> &str { "planner" }

    fn system_prompt(&self) -> &str { PLANNER_SYSTEM_PROMPT }

    fn max_iterations(&self) -> usize { PLANNER_MAX_ITERATIONS }

    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let registry = create_subagent_registry(self.working_directory.clone());
        let tools = registry.to_gemini_tools(&crate::mode::Mode::Plan);
        let tool_config = Some(crate::tool::registry::ToolRegistry::tool_config());
        let system_instruction = Content::system(self.system_prompt());

        let mut user_message = format!("Task: {}\n", request.task);
        if !request.context.is_empty() {
            user_message.push_str("\nContext:\n");
            for ctx in &request.context {
                user_message.push_str(&format!("- {}\n", ctx));
            }
        }
        user_message.push_str(&format!(
            "\nWorking directory: {}",
            request.working_directory
        ));

        let mut history = vec![Content::user(&user_message)];

        let result = tokio::time::timeout(
            Duration::from_secs(PLANNER_TIMEOUT_SECS),
            self.run_subagent_loop(
                client, &mut history, system_instruction, tools, tool_config,
            ),
        )
        .await
        .map_err(|_| ClosedCodeError::AgentTimeout {
            agent_id: self.agent_type().into(),
            seconds: PLANNER_TIMEOUT_SECS,
        })??;

        match result {
            Some(mut response) => {
                response.request_id = request.id;
                Ok(response)
            }
            None => Ok(AgentResponse {
                request_id: request.id,
                agent_type: self.agent_type().into(),
                summary: "Planner completed without a structured plan.".into(),
                detailed_report: "The planner exhausted iterations without \
                    calling create_report.".into(),
                artifacts: Vec::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_agent_properties() {
        let agent = PlannerAgent::new(PathBuf::from("/tmp"));
        assert_eq!(agent.agent_type(), "planner");
        assert_eq!(agent.max_iterations(), 15);
        assert!(agent.system_prompt().contains("software architect"));
    }

    #[test]
    fn extract_report_includes_plan_artifact() {
        let args = serde_json::json!({
            "summary": "3-step plan",
            "detailed_report": "Step 1: ...\nStep 2: ...\nStep 3: ...",
        });
        let report = PlannerAgent::extract_report(&args).unwrap();
        assert_eq!(report.artifacts.len(), 1);
        assert!(matches!(
            report.artifacts[0].artifact_type,
            ArtifactType::Plan
        ));
    }
}
```

### `src/agent/web_searcher.rs`

The WebSearchAgent uses Gemini's native google_search grounding. It makes a single non-streaming API call — no tool loop, no filesystem tools.

```rust
use async_trait::async_trait;
use std::time::Duration;

use crate::agent::{Agent, AgentRequest, AgentResponse};
use crate::agent::message::{
    Artifact, ArtifactType, WebSource as AgentWebSource,
};
use crate::error::{ClosedCodeError, Result};
use crate::gemini::GeminiClient;
use crate::gemini::types::*;

const WEB_SEARCH_TIMEOUT_SECS: u64 = 30;

const WEB_SEARCH_SYSTEM_PROMPT: &str = "\
You are a web research agent. Search the web to find relevant, up-to-date information \
about the given topic. Focus on:

1. Official documentation and best practices.
2. Recent blog posts, tutorials, and Stack Overflow answers.
3. Library READMEs and changelogs for version-specific info.

Synthesize findings into a clear, actionable summary. Always cite your sources. \
Present information in a structured format with headings for different aspects.";

#[derive(Debug)]
pub struct WebSearchAgent;

impl WebSearchAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Agent for WebSearchAgent {
    fn agent_type(&self) -> &str { "web_searcher" }

    fn system_prompt(&self) -> &str { WEB_SEARCH_SYSTEM_PROMPT }

    fn max_iterations(&self) -> usize { 1 } // Single request-response

    async fn run(
        &self,
        client: &GeminiClient,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let mut user_message = format!("Research topic: {}\n", request.task);
        if !request.context.is_empty() {
            user_message.push_str("\nContext:\n");
            for ctx in &request.context {
                user_message.push_str(&format!("- {}\n", ctx));
            }
        }

        let api_request = GenerateContentRequest {
            contents: vec![Content::user(&user_message)],
            system_instruction: Some(Content::system(self.system_prompt())),
            generation_config: Some(GenerationConfig {
                temperature: Some(0.7),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(4096),
            }),
            // google_search grounding — NOT function calling
            tools: Some(vec![GeminiTool::GoogleSearch(GoogleSearchTool::new())]),
            tool_config: None, // No function calling config for google_search
        };

        let response = tokio::time::timeout(
            Duration::from_secs(WEB_SEARCH_TIMEOUT_SECS),
            client.generate_content(&api_request),
        )
        .await
        .map_err(|_| ClosedCodeError::AgentTimeout {
            agent_id: "web_searcher".into(),
            seconds: WEB_SEARCH_TIMEOUT_SECS,
        })??;

        let candidate = response.candidates.first()
            .ok_or(ClosedCodeError::EmptyResponse)?;

        let text = candidate.content.as_ref()
            .and_then(|c| c.parts.first())
            .and_then(|p| match p {
                Part::Text(t) => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "No results found.".into());

        // Extract grounding sources
        let sources: Vec<AgentWebSource> = candidate.grounding_metadata
            .as_ref()
            .map(|gm| {
                gm.grounding_chunks.iter().filter_map(|chunk| {
                    chunk.web.as_ref().map(|web| AgentWebSource {
                        url: web.uri.clone(),
                        title: web.title.clone(),
                    })
                }).collect()
            })
            .unwrap_or_default();

        let summary = if text.len() > 200 {
            format!("{}...", &text[..200])
        } else {
            text.clone()
        };

        let mut artifacts = Vec::new();
        if !sources.is_empty() {
            artifacts.push(Artifact {
                name: "Web Search Results".into(),
                artifact_type: ArtifactType::WebSearchResults {
                    sources: sources.clone(),
                },
                content: text.clone(),
            });
        }

        Ok(AgentResponse {
            request_id: request.id,
            agent_type: "web_searcher".into(),
            summary,
            detailed_report: text,
            artifacts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_agent_properties() {
        let agent = WebSearchAgent::new();
        assert_eq!(agent.agent_type(), "web_searcher");
        assert_eq!(agent.max_iterations(), 1);
        assert!(agent.system_prompt().contains("web research"));
    }
}
```

### `src/tool/report.rs`

The CreateReportTool is a special tool used by sub-agents to structure their output. When called, the sub-agent's loop detects it by name and extracts the report.

```rust
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;
use super::{Tool, ParamBuilder};

/// Special tool for sub-agents to structure their final output.
///
/// When the sub-agent's tool-call loop detects a call to "create_report",
/// it extracts the arguments as the agent's response and terminates the loop.
/// The execute() method is a fallback that should not normally be reached.
#[derive(Debug)]
pub struct CreateReportTool;

impl CreateReportTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CreateReportTool {
    fn name(&self) -> &str { "create_report" }

    fn description(&self) -> &str {
        "Submit your research findings as a structured report. Call this when you have \
         gathered enough information to answer the task. This is REQUIRED — you must \
         call this tool to deliver your results."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "summary",
                    "A brief 1-3 sentence summary of your findings.",
                    true,
                )
                .string(
                    "detailed_report",
                    "The full detailed report with your findings, analysis, \
                     and recommendations. Use markdown formatting. Be thorough.",
                    true,
                )
                .string(
                    "code_snippets",
                    "Optional JSON array of code snippets: \
                     [{\"name\": \"file.rs\", \"language\": \"rust\", \
                     \"content\": \"...\"}]. Include relevant code you found \
                     or propose.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, _args: Value) -> Result<Value> {
        // This should never be called directly — the sub-agent loop
        // intercepts create_report calls before reaching execute().
        // If somehow reached, return a success response.
        Ok(json!({
            "status": "report_received",
            "note": "Report was processed by the sub-agent framework."
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        // Available in all modes (sub-agents use it internally)
        vec![Mode::Explore, Mode::Plan, Mode::Execute]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_report_tool_properties() {
        let tool = CreateReportTool::new();
        assert_eq!(tool.name(), "create_report");
        assert!(tool.description().contains("structured report"));
    }

    #[test]
    fn create_report_declaration_has_required_params() {
        let tool = CreateReportTool::new();
        let decl = tool.declaration();
        assert_eq!(decl.name, "create_report");
        assert!(decl.parameters.required.as_ref().unwrap().contains(&"summary".to_string()));
        assert!(decl.parameters.required.as_ref().unwrap().contains(&"detailed_report".to_string()));
    }

    #[tokio::test]
    async fn create_report_execute_fallback() {
        let tool = CreateReportTool::new();
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["status"], "report_received");
    }
}
```

**How create_report terminates the sub-agent loop**: The sub-agent's `run_subagent_loop` method iterates through function calls. Before executing each call via the registry, it checks if the function name is `"create_report"`. If so, it calls `extract_report(args)` to parse the arguments into an `AgentResponse` and returns immediately, breaking out of the loop. The `execute()` method on the Tool is a dead-code fallback that returns a success JSON if somehow reached.

### `src/tool/spawn.rs`

Spawn tools are Tool trait implementations that create and run sub-agents. They are registered in the orchestrator's ToolRegistry and invoked by the main LLM as function calls.

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::explorer::ExplorerAgent;
use crate::agent::message::AgentRequest;
use crate::agent::planner::PlannerAgent;
use crate::agent::web_searcher::WebSearchAgent;
use crate::agent::Agent;
use crate::error::Result;
use crate::gemini::GeminiClient;
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;
use super::{Tool, ParamBuilder};

// ── SpawnExplorerTool ──

#[derive(Debug)]
pub struct SpawnExplorerTool {
    client: Arc<GeminiClient>,
    working_directory: PathBuf,
}

impl SpawnExplorerTool {
    pub fn new(client: Arc<GeminiClient>, working_directory: PathBuf) -> Self {
        Self { client, working_directory }
    }
}

#[async_trait]
impl Tool for SpawnExplorerTool {
    fn name(&self) -> &str { "spawn_explorer" }

    fn description(&self) -> &str {
        "Spawn an explorer sub-agent to research the codebase. The explorer will \
         autonomously read files, search for patterns, and produce a structured \
         report. Use this when you need to understand code architecture, find \
         implementations, or analyze patterns before answering the user."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "task",
                    "A clear description of what the explorer should research. \
                     Be specific: name files, patterns, or questions.",
                    true,
                )
                .string(
                    "context",
                    "Optional context from the current conversation that would \
                     help the explorer understand what is needed.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let task = args["task"].as_str()
            .unwrap_or("Explore the codebase")
            .to_string();
        let context_str = args["context"].as_str().unwrap_or("");

        let mut request = AgentRequest::new(
            task,
            self.working_directory.to_string_lossy().to_string(),
        );
        if !context_str.is_empty() {
            request = request.with_context(vec![context_str.to_string()]);
        }

        tracing::info!("Spawning explorer agent: {}", request.task);

        let agent = ExplorerAgent::new(self.working_directory.clone());
        let response = agent.run(&self.client, request).await?;

        // Serialize the response as the tool's return value
        Ok(json!({
            "agent_type": response.agent_type,
            "summary": response.summary,
            "detailed_report": response.detailed_report,
            "artifact_count": response.artifacts.len(),
            "artifacts": response.artifacts.iter().map(|a| {
                json!({
                    "name": a.name,
                    "content": a.content,
                })
            }).collect::<Vec<_>>(),
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Explore, Mode::Plan, Mode::Execute]
    }
}

// ── SpawnPlannerTool ──

#[derive(Debug)]
pub struct SpawnPlannerTool {
    client: Arc<GeminiClient>,
    working_directory: PathBuf,
}

impl SpawnPlannerTool {
    pub fn new(client: Arc<GeminiClient>, working_directory: PathBuf) -> Self {
        Self { client, working_directory }
    }
}

#[async_trait]
impl Tool for SpawnPlannerTool {
    fn name(&self) -> &str { "spawn_planner" }

    fn description(&self) -> &str {
        "Spawn a planner sub-agent to create a structured implementation plan. \
         The planner analyzes the codebase and produces step-by-step plans with \
         affected files, patterns to follow, and risk assessments."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "task",
                    "A clear description of what needs to be planned. \
                     Include goals, constraints, and any known requirements.",
                    true,
                )
                .string(
                    "context",
                    "Optional context from the current conversation.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let task = args["task"].as_str()
            .unwrap_or("Create an implementation plan")
            .to_string();
        let context_str = args["context"].as_str().unwrap_or("");

        let mut request = AgentRequest::new(
            task,
            self.working_directory.to_string_lossy().to_string(),
        );
        if !context_str.is_empty() {
            request = request.with_context(vec![context_str.to_string()]);
        }

        tracing::info!("Spawning planner agent: {}", request.task);

        let agent = PlannerAgent::new(self.working_directory.clone());
        let response = agent.run(&self.client, request).await?;

        Ok(json!({
            "agent_type": response.agent_type,
            "summary": response.summary,
            "detailed_report": response.detailed_report,
            "artifact_count": response.artifacts.len(),
            "artifacts": response.artifacts.iter().map(|a| {
                json!({
                    "name": a.name,
                    "content": a.content,
                })
            }).collect::<Vec<_>>(),
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Plan]
    }
}

// ── SpawnWebSearchTool ──

#[derive(Debug)]
pub struct SpawnWebSearchTool {
    client: Arc<GeminiClient>,
    working_directory: PathBuf,
}

impl SpawnWebSearchTool {
    pub fn new(client: Arc<GeminiClient>, working_directory: PathBuf) -> Self {
        Self { client, working_directory }
    }
}

#[async_trait]
impl Tool for SpawnWebSearchTool {
    fn name(&self) -> &str { "spawn_web_search" }

    fn description(&self) -> &str {
        "Spawn a web search sub-agent to research a topic online. \
         Uses Google Search grounding to find recent documentation, \
         best practices, and solutions. Returns findings with sources."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "query",
                    "The research query. Be specific and include relevant \
                     technology names and version numbers.",
                    true,
                )
                .string(
                    "context",
                    "Optional context about why this search is needed.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let query = args["query"].as_str()
            .unwrap_or("Search the web")
            .to_string();
        let context_str = args["context"].as_str().unwrap_or("");

        let mut request = AgentRequest::new(
            query,
            self.working_directory.to_string_lossy().to_string(),
        );
        if !context_str.is_empty() {
            request = request.with_context(vec![context_str.to_string()]);
        }

        tracing::info!("Spawning web search agent: {}", request.task);

        let agent = WebSearchAgent::new();
        let response = agent.run(&self.client, request).await?;

        Ok(json!({
            "agent_type": response.agent_type,
            "summary": response.summary,
            "detailed_report": response.detailed_report,
            "artifact_count": response.artifacts.len(),
        }))
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Plan]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_explorer_tool_properties() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let tool = SpawnExplorerTool::new(client, PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "spawn_explorer");
        assert!(tool.available_modes().contains(&Mode::Explore));
        assert!(tool.available_modes().contains(&Mode::Plan));
        assert!(tool.available_modes().contains(&Mode::Execute));
    }

    #[test]
    fn spawn_planner_tool_plan_mode_only() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let tool = SpawnPlannerTool::new(client, PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "spawn_planner");
        assert_eq!(tool.available_modes(), vec![Mode::Plan]);
    }

    #[test]
    fn spawn_web_search_tool_plan_mode_only() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let tool = SpawnWebSearchTool::new(client, PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "spawn_web_search");
        assert_eq!(tool.available_modes(), vec![Mode::Plan]);
    }
}
```

### `src/agent/orchestrator.rs`

The Orchestrator replaces the direct API call in the REPL. It manages the main conversation, dispatches to sub-agents via spawn tools, handles context window pruning, and provides mode-specific tool sets.

```rust
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::Config;
use crate::error::{ClosedCodeError, Result};
use crate::gemini::GeminiClient;
use crate::gemini::types::*;
use crate::gemini::stream::{consume_stream, StreamEvent, StreamResult};
use crate::mode::Mode;
use crate::tool::registry::{create_orchestrator_registry, ToolRegistry};

const MAX_ORCHESTRATOR_ITERATIONS: usize = 30;
const MAX_CONTEXT_TURNS: usize = 50;

pub struct Orchestrator {
    client: Arc<GeminiClient>,
    mode: Mode,
    working_directory: PathBuf,
    history: Vec<Content>,
    registry: ToolRegistry,
}

impl Orchestrator {
    pub fn new(
        client: Arc<GeminiClient>,
        mode: Mode,
        working_directory: PathBuf,
    ) -> Self {
        let registry = create_orchestrator_registry(
            working_directory.clone(),
            &mode,
            client.clone(),
        );

        Self {
            client,
            mode,
            working_directory,
            history: Vec::new(),
            registry,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    /// Clear conversation history.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Get current turn count.
    pub fn turn_count(&self) -> usize {
        self.history.len()
    }

    /// Build the mode-specific system prompt.
    fn system_prompt(&self) -> String {
        let base = format!(
            "You are closed-code, an AI coding assistant operating in {} mode.\n\
             Working directory: {}\n\n",
            self.mode,
            self.working_directory.display()
        );

        let mode_instructions = match self.mode {
            Mode::Explore => "\
In Explore mode, you help users understand codebases. You can:
- Read files, search for patterns, and list directories directly.
- Spawn an explorer sub-agent for deep codebase research.

Use spawn_explorer when the user's question requires reading multiple \
files or understanding broad architectural patterns. For simple questions \
about a single file, use the filesystem tools directly.

When you receive an explorer's report, synthesize it into a clear, \
natural response. Do not just dump the raw report.",

            Mode::Plan => "\
In Plan mode, you help users design and plan code changes. You can:
- Read files and explore the codebase directly.
- Spawn an explorer for codebase research.
- Spawn a planner for structured implementation plans.
- Spawn a web search for documentation and best practices.

For planning tasks, consider spawning both an explorer (to understand \
the current code) and a web search (for best practices), then synthesize \
the results. For complex plans, use spawn_planner.

Present plans in a clear, actionable format with numbered steps.",

            Mode::Execute => "\
In Execute mode, you help users modify code. You can:
- Read files and explore the codebase.
- Spawn an explorer for research.
- (Phase 4: write/edit files with approval.)

Currently, file write tools are not yet available (coming in Phase 4). \
You can still explore and explain code.",
        };

        format!("{}{}", base, mode_instructions)
    }

    /// Prune conversation history when it exceeds MAX_CONTEXT_TURNS.
    /// Strategy: keep the most recent N turns, drop the oldest.
    fn prune_history(&mut self) {
        if self.history.len() <= MAX_CONTEXT_TURNS {
            return;
        }

        let prune_count = self.history.len() - MAX_CONTEXT_TURNS;
        tracing::info!(
            "Context window pruning: removing {} oldest turns (keeping {})",
            prune_count,
            MAX_CONTEXT_TURNS,
        );

        // Remove from the front, keeping the most recent turns
        let mut removed = 0;
        while removed < prune_count && !self.history.is_empty() {
            self.history.remove(0);
            removed += 1;
        }

        // Ensure the first message has role "user" (Gemini requires this)
        while !self.history.is_empty() {
            if let Some(role) = &self.history[0].role {
                if role == "user" {
                    break;
                }
            }
            self.history.remove(0);
        }
    }

    /// Handle a user input message. This is the main entry point from the REPL.
    ///
    /// Flow:
    /// 1. Add user message to history.
    /// 2. Send request via streaming for the first call (user-visible text).
    /// 3. If function calls are returned, enter the non-streaming tool loop.
    /// 4. Repeat until text-only response or max iterations.
    /// 5. Prune history if needed.
    pub async fn handle_user_input(
        &mut self,
        input: &str,
        config: &Config,
        on_text: impl Fn(&str),
        on_tool: impl Fn(&str),
    ) -> Result<String> {
        self.history.push(Content::user(input));
        self.prune_history();

        let system_instruction = Content::system(&self.system_prompt());
        let tools = self.registry.to_gemini_tools(&self.mode);
        let tool_config = if tools.is_some() {
            Some(ToolRegistry::tool_config())
        } else {
            None
        };

        // First request: streaming for immediate text display
        let request = GenerateContentRequest {
            contents: self.history.clone(),
            system_instruction: Some(system_instruction.clone()),
            generation_config: Some(GenerationConfig {
                temperature: Some(1.0),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(config.max_output_tokens),
            }),
            tools: tools.clone(),
            tool_config: tool_config.clone(),
        };

        let es = self.client.stream_generate_content(&request);

        let stream_result = consume_stream(es, |event| {
            match event {
                StreamEvent::TextDelta(text) => on_text(&text),
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
                StreamEvent::FunctionCall(_) => {}
            }
        }).await?;

        match stream_result {
            StreamResult::Text(text) => {
                self.history.push(Content::model(&text));
                Ok(text)
            }
            StreamResult::FunctionCall { text_so_far, response } => {
                // Enter tool-call loop
                if let Some(candidate) = response.candidates.first() {
                    if let Some(content) = &candidate.content {
                        self.history.push(content.clone());
                    }
                }

                // Execute initial function calls
                let mut response_parts = Vec::new();
                for part in response.function_calls() {
                    if let Part::FunctionCall { name, args } = part {
                        on_tool(&format!("Using {}...", name));
                        let result = match self.registry
                            .execute(name, args.clone()).await
                        {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("Tool '{}' failed: {}", name, e);
                                serde_json::json!({"error": e.to_string()})
                            }
                        };
                        response_parts.push(Part::FunctionResponse {
                            name: name.clone(),
                            response: result,
                        });
                    }
                }
                self.history.push(Content::function_responses(response_parts));

                // Continue with non-streaming tool loop
                let final_text = self.run_orchestrator_loop(
                    system_instruction,
                    config,
                    tools,
                    tool_config,
                    &on_text,
                    &on_tool,
                ).await?;

                let mut combined = text_so_far;
                combined.push_str(&final_text);
                Ok(combined)
            }
        }
    }

    /// Non-streaming tool-call loop for the orchestrator.
    /// Continues after the initial streaming request detected function calls.
    async fn run_orchestrator_loop(
        &mut self,
        system_instruction: Content,
        config: &Config,
        tools: Option<Vec<GeminiTool>>,
        tool_config: Option<ToolConfig>,
        on_text: &impl Fn(&str),
        on_tool: &impl Fn(&str),
    ) -> Result<String> {
        let mut final_text = String::new();

        for iteration in 0..MAX_ORCHESTRATOR_ITERATIONS {
            tracing::debug!(
                "Orchestrator loop iteration {}/{}",
                iteration + 1,
                MAX_ORCHESTRATOR_ITERATIONS,
            );

            let request = GenerateContentRequest {
                contents: self.history.clone(),
                system_instruction: Some(system_instruction.clone()),
                generation_config: Some(GenerationConfig {
                    temperature: Some(1.0),
                    top_p: None,
                    top_k: None,
                    max_output_tokens: Some(config.max_output_tokens),
                }),
                tools: tools.clone(),
                tool_config: tool_config.clone(),
            };

            let response = self.client.generate_content(&request).await?;

            let candidate = response.candidates.first()
                .ok_or(ClosedCodeError::EmptyResponse)?;
            let content = candidate.content.as_ref()
                .ok_or(ClosedCodeError::EmptyResponse)?;

            // Check for safety block
            if candidate.finish_reason.as_deref() == Some("SAFETY") {
                let reason = candidate.safety_ratings.iter()
                    .filter(|r| r.probability == "HIGH"
                        || r.probability == "MEDIUM")
                    .map(|r| r.category.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(ClosedCodeError::SafetyBlocked { reason });
            }

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

            if !text_parts.is_empty() {
                let text = text_parts.join("");
                on_text(&text);
                final_text.push_str(&text);
            }

            if function_calls.is_empty() {
                self.history.push(content.clone());
                break;
            }

            self.history.push(content.clone());

            let mut response_parts = Vec::new();
            for (name, args) in &function_calls {
                on_tool(&format!("Using {}...", name));

                let result = match self.registry
                    .execute(name, args.clone()).await
                {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("Tool '{}' failed: {}", name, e);
                        serde_json::json!({"error": e.to_string()})
                    }
                };

                response_parts.push(Part::FunctionResponse {
                    name: name.clone(),
                    response: result,
                });
            }

            self.history.push(Content::function_responses(response_parts));
        }

        Ok(final_text)
    }
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("mode", &self.mode)
            .field("working_directory", &self.working_directory)
            .field("history_len", &self.history.len())
            .field("tool_count", &self.registry.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_system_prompt_explore() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let orch = Orchestrator::new(
            client,
            Mode::Explore,
            PathBuf::from("/tmp/project"),
        );
        let prompt = orch.system_prompt();
        assert!(prompt.contains("explore"));
        assert!(prompt.contains("/tmp/project"));
        assert!(prompt.contains("spawn_explorer"));
    }

    #[test]
    fn orchestrator_system_prompt_plan() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let orch = Orchestrator::new(
            client,
            Mode::Plan,
            PathBuf::from("/tmp"),
        );
        let prompt = orch.system_prompt();
        assert!(prompt.contains("plan"));
        assert!(prompt.contains("spawn_planner"));
        assert!(prompt.contains("web search"));
    }

    #[test]
    fn orchestrator_clear_history() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let mut orch = Orchestrator::new(
            client,
            Mode::Explore,
            PathBuf::from("/tmp"),
        );
        orch.history.push(Content::user("test"));
        assert_eq!(orch.turn_count(), 1);
        orch.clear_history();
        assert_eq!(orch.turn_count(), 0);
    }

    #[test]
    fn orchestrator_prune_history() {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let mut orch = Orchestrator::new(
            client,
            Mode::Explore,
            PathBuf::from("/tmp"),
        );

        // Add more than MAX_CONTEXT_TURNS messages
        for i in 0..60 {
            if i % 2 == 0 {
                orch.history.push(Content::user(&format!("msg {}", i)));
            } else {
                orch.history.push(Content::model(&format!("reply {}", i)));
            }
        }

        assert_eq!(orch.history.len(), 60);
        orch.prune_history();
        assert!(orch.history.len() <= MAX_CONTEXT_TURNS);

        // First message should be a "user" message
        assert_eq!(
            orch.history[0].role.as_deref(),
            Some("user"),
        );
    }
}
```

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Sub-agents share GeminiClient via `Arc`** | Sub-agents make their own API calls but share the underlying `reqwest::Client` and configuration. `Arc<GeminiClient>` is cheap to clone and avoids duplicating the HTTP connection pool. No `Mutex` needed because `GeminiClient` methods take `&self` (immutable). |
| **`create_report` detected by name, not by special return** | The sub-agent loop checks `if name == "create_report"` before calling `registry.execute()`. This is simpler than adding a return channel or special `ToolResult` enum. The `execute()` method on `CreateReportTool` is dead code — a fallback that returns a success JSON if somehow reached. |
| **Sub-agent results serialized as JSON in function response** | The orchestrator's LLM receives the sub-agent's `AgentResponse` as the `functionResponse.response` JSON. This lets the LLM read the summary, detailed report, and artifacts naturally and synthesize them into a user-facing response. The LLM is better at presenting information than raw string formatting. |
| **Same model for sub-agents and orchestrator** | Simplicity. Sub-agents share the same `GeminiClient` instance, which uses the model specified at startup. A future enhancement (Phase 5 config) could allow configuring a cheaper model (e.g., `gemini-2.5-flash`) for sub-agents via `[agents] model = "gemini-2.5-flash"`. For now, one model for all. |
| **Context pruning drops oldest turns** | Simple and predictable. When history exceeds 50 turns, the oldest messages are removed. The algorithm ensures the first remaining message has `role: "user"` (Gemini requirement). Phase 8 adds smarter pruning via the `/compact` command (LLM-based summarization). |
| **120-second timeout for sub-agents** | Sub-agents can make up to 15 API calls, each potentially slow. 120 seconds provides ample time while preventing infinite hangs. The timeout wraps the entire `run_subagent_loop` call. On timeout, the orchestrator receives `AgentTimeout` error and can report it to the user. |
| **Sub-agents cannot spawn other sub-agents** | Prevents recursive spawning (and potential infinite loops or exponential resource consumption). The sub-agent registry (`create_subagent_registry`) includes filesystem tools and `create_report` but excludes all `spawn_*` tools. If deep research is needed, the orchestrator can spawn multiple sub-agents sequentially. |
| **Non-streaming for sub-agents** | Sub-agent output is not user-facing — it goes back to the orchestrator as a function response. Streaming would add complexity for no user benefit. Only the orchestrator's initial request uses streaming (for immediate text display to the user). |
| **`GeminiTool` enum with `#[serde(untagged)]`** | The Gemini API `tools` array can contain either `{"functionDeclarations": [...]}` or `{"google_search": {}}`. Using an untagged enum serializes correctly to either format. The `WebSearchAgent` uses `GeminiTool::GoogleSearch`, while all other contexts use `GeminiTool::Functions`. |
| **Orchestrator owns `ToolRegistry`** | The registry depends on mode and requires `Arc<GeminiClient>`. Having the `Orchestrator` own it keeps configuration self-contained. Mode changes (future `/explore`, `/plan` commands) can recreate the registry. |
| **Fallback when sub-agent does not call `create_report`** | If the sub-agent exhausts its 15 iterations without calling `create_report`, the agent returns a minimal `AgentResponse` with a warning message. If the agent produces a text-only response (no function calls), that text is captured as the report. This ensures the orchestrator always gets a usable response. |
| **Max 30 iterations for orchestrator** (up from Phase 2's 10) | The orchestrator may need to: spawn explorer (1 call), process report (1 call), spawn planner (1 call), process report (1 call), use direct tools (several calls), produce final response. 30 iterations accommodates this multi-agent workflow. Phase 2's `run_tool_loop` used 10 iterations for simpler single-level tool usage. |

---

## Milestone / Verification

After implementing Phase 3, verify each capability:

```bash
# 1. Orchestrator replaces direct REPL (tools shown at startup)
cargo run
# Expected:
#   closed-code
#   Mode: explore | Model: gemini-3.1-pro-preview | Tools: 6
#   Type /help for commands, /quit to exit.
# (6 tools = 5 filesystem/shell + 1 spawn_explorer)

# 2. Direct tool usage still works (Phase 2 functionality preserved)
# explore > What files are in this project?
# ⠋ Using list_directory...
# This project has the following files: ...

# 3. Explorer sub-agent can be spawned
# explore > Analyze this project's architecture in detail
# ⠋ Using spawn_explorer...
# (wait while sub-agent runs internally — may take 10-30 seconds)
# Based on the explorer's analysis, this project is structured as follows:
# (orchestrator synthesizes the explorer's report into natural text)

# 4. Plan mode has additional spawn tools
cargo run --mode plan
# Expected: Tools: 8
# (5 filesystem/shell + spawn_explorer + spawn_planner + spawn_web_search)

# 5. Planner sub-agent creates structured plans
# plan > How should I add caching to the Gemini API client?
# ⠋ Using spawn_explorer...
# ⠋ Using spawn_planner...
# Here's a plan for adding caching:
# 1. Create src/cache/mod.rs with a CacheLayer trait...
# 2. Implement LruCache in src/cache/lru.rs...

# 6. Web search agent returns grounded results
# plan > What are the best Rust caching crates in 2026?
# ⠋ Using spawn_web_search...
# Based on current web research:
# - moka is the most popular async-compatible cache...
# Sources: [moka docs](https://...), [comparison article](https://...)

# 7. Context window pruning works
# (After many turns, history is automatically pruned)
# Expected: tracing::info log "Context window pruning: removing 10 oldest turns"

# 8. Sub-agent timeout is handled gracefully
# (If a sub-agent hangs, after 120s:)
# Error: Agent 'explorer' timed out after 120s

# 9. Sub-agent errors are returned to the model, not crashes
# (If the explorer encounters an error during research:)
# The explorer encountered an issue but here's what it found...
# (model recovers gracefully from partial results)

# 10. Mode-specific tool availability
cargo run --mode explore
# explore > (LLM tries to call spawn_planner)
# Error: Tool 'spawn_planner' not found
# (spawn_planner is only available in Plan mode)

# 11. Execute mode has explorer but not planner/web_search
cargo run --mode execute
# Expected: Tools: 6 (5 filesystem/shell + spawn_explorer)

# 12. Verbose mode shows sub-agent activity
RUST_LOG=debug cargo run
# Expected: debug logs showing:
#   "Spawning explorer agent: ..."
#   "Explorer agent loop iteration 1/15"
#   "Explorer agent loop iteration 2/15"
#   ...
```

---

## What This Phase Does NOT Include

These are explicitly deferred to later phases:

- **File writes / edits** (Phase 4) — orchestrator has no write tools, even in Execute mode
- **TOML config** (Phase 5) — sub-agent model selection, iteration limits, timeouts are all hardcoded
- **Sub-agent model override** (Phase 5) — no `[agents] model = "gemini-2.5-flash"` config option yet
- **Session persistence** (Phase 8) — sub-agent history is in-memory only; not saved to JSONL
- **Parallel sub-agent execution** (Phase 10) — sub-agents run sequentially when multiple are spawned; `tokio::join!` for concurrent sub-agents is Phase 10
- **Sub-agent progress display** (Phase 9/10) — no real-time "Explorer: 3/15 tool calls" in the TUI
- **Git worktree isolation** (Phase 10) — sub-agents share the same working directory
- **Sub-agent streaming** (Phase 9) — all sub-agent API calls are non-streaming
- **MCP tool integration** (Phase 8) — sub-agents cannot use MCP server tools
- **Smart context pruning** (Phase 8) — pruning is simple "drop oldest"; LLM-based summarization is the Phase 8 `/compact` command

---

*See [phase_2.md](phase_2.md) for the tool system this phase builds on, and [phase_spec.md](phase_spec.md) for the full 10-phase roadmap.*

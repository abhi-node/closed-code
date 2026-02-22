# closed-code: Implementation Phase Specification

10 phases from zero to Codex CLI feature parity. Each phase produces a usable, demo-able binary. The tool grows incrementally — from a basic streaming chat, through agentic codebase exploration, to a full-screen TUI with sandboxing, MCP, multi-agent parallel execution, and polished animations.

**Reference**: See `docs/README.md` for the core architecture spec (traits, tool definitions, API types, project structure).

---

## Phase Dependency Graph

```
Phase 1 (Foundation + Gemini + REPL)
  │
  ▼
Phase 2 (Tool System + Filesystem + Tool-Call Loop)
  │
  ▼
Phase 3 (Sub-Agent Architecture)
  │
  ▼
Phase 4 (Execute Mode — Diffs + Approvals)
  │
  ▼
Phase 5 (Configuration + Enhanced REPL)
  │
  ├───► Phase 6 (Git Integration)        ┐
  │                                       │ can develop
  ├───► Phase 7 (Sandboxing)              │ in parallel
  │                                       │
  ├───► Phase 8 (Sessions + MCP + Images) ┘
  │
  ▼
Phase 9 (Full-Screen TUI with Ratatui)  ← requires 6, 7, 8
  │
  ▼
Phase 10 (Polish, Animations, Multi-Agent, Parity)
```

---

## Phase Summary

| Phase | Name | Key Deliverable | After This Phase | Est. Lines |
|-------|------|----------------|------------------|------------|
| 1 | Foundation + Gemini + REPL | Streaming conversation | Chat with Gemini in terminal | ~3,000 |
| 2 | Tool System + Filesystem | LLM reads codebase | Explore code via LLM | ~2,000 |
| 3 | Sub-Agent Architecture | Multi-agent research | Delegate research to sub-agents | ~2,500 |
| 4 | Execute Mode + Diffs | Code modification with approval | Write code with diff review | ~2,000 |
| 5 | Config + Enhanced REPL | Full config, slash commands | Production-ready CLI | ~1,500 |
| 6 | Git Integration | Branch awareness, /diff, /review | Git-aware coding assistant | ~1,200 |
| 7 | Sandboxing | Platform-specific security | Safe autonomous execution | ~2,000 |
| 8 | Sessions + MCP | Persistence, external tools | Resume conversations, extend tools | ~2,500 |
| 9 | Full-Screen TUI | Ratatui interface | Visual parity with Codex CLI | ~3,500 |
| 10 | Polish + Multi-Agent | Animations, parallel agents | Full feature parity | ~3,000 |
| | | | **Total** | **~23,200** |

---

## Phase 1: Foundation + Gemini Client + Basic Conversation

**Goal**: A working binary that holds a multi-turn streaming conversation with Gemini 3.1 Pro Preview.

### Files

```
Cargo.toml
src/
  main.rs              # Entry point, tokio runtime, CLI dispatch
  cli.rs               # Clap derive: --mode, --directory, --api-key, --model, --verbose, ask subcommand
  config.rs            # Config struct from env + CLI flags (no TOML yet)
  error.rs             # ClosedCodeError enum (thiserror): ApiError, ConfigError, IoError, ParseError, etc.
  mode/
    mod.rs             # Mode enum (Explore, Plan, Execute) with Display/FromStr
  gemini/
    mod.rs             # Module re-exports
    types.rs           # Full Gemini API serde types (see below)
    client.rs          # GeminiClient: generate_content(), stream_generate_content(), retry logic
    stream.rs          # SSE parser: data: lines → GenerateContentResponse stream
  ui/
    mod.rs             # Module re-exports
    theme.rs           # ANSI color constants (user, assistant, error, success, dim, accent)
    spinner.rs         # indicatif spinner for "Thinking..."
  repl.rs              # Minimal REPL: input loop, conversation history, streaming display, /help /quit /clear
```

### Key Implementation Details

**Gemini API types** (`gemini/types.rs`):
- `GenerateContentRequest` — contents, system_instruction, tools, generation_config, tool_config
- `Content` — role ("user"/"model"), parts vec
- `Part` enum — Text, FunctionCall, FunctionResponse, InlineData
- **Custom `Part` deserializer**: Gemini returns camelCase JSON (`functionCall`, `functionResponse`). Deserializer checks which key exists in the JSON object rather than using `#[serde(untagged)]`
- `FunctionDeclaration` — name, description, parameters (JSON Schema)
- `GenerateContentResponse` — candidates, usage_metadata
- `UsageMetadata` — prompt_token_count, candidates_token_count, total_token_count

**GeminiClient** (`gemini/client.rs`):
- Constructor: `new(api_key, model)` with base URL `https://generativelanguage.googleapis.com/v1beta`
- `generate_content(request)` — non-streaming POST to `:generateContent?key={key}`
- `stream_generate_content(request)` — SSE POST to `:streamGenerateContent?alt=sse&key={key}`
- **Retry**: Exponential backoff (500ms → 1s → 2s) for 429 and 5xx, max 3 attempts
- Manual `Debug` impl that redacts the API key

**SSE Parser** (`gemini/stream.rs`):
- Uses `reqwest-eventsource` to consume the SSE stream
- Each `data:` line is parsed as a `GenerateContentResponse`
- Yields text chunks for streaming display
- When a `functionCall` part is detected: switches to buffering mode, collects the complete response

**REPL** (`repl.rs`):
- Reads lines from stdin via crossterm raw mode
- Maintains `Vec<Content>` as conversation history
- On user input: append user Content → call streaming endpoint → print tokens as they arrive → append assistant Content
- Ctrl+C cancels current generation, Ctrl+D exits
- Slash commands: `/help`, `/quit`, `/clear`

### Dependencies

```toml
[package]
name = "closed-code"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive", "env"] }
reqwest = { version = "0.12", features = ["json", "stream"] }
reqwest-eventsource = "0.6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
crossterm = "0.28"
indicatif = "0.17"
thiserror = "2"
anyhow = "1"
async-trait = "0.1"
tokio-stream = "0.1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v4", "serde"] }
dirs = "6"
```

### Milestone

```bash
# CLI help works
cargo run -- --help

# One-shot streaming query
export GEMINI_API_KEY="your-key"
cargo run -- ask "What is Rust?"
# → Streams response token-by-token

# Multi-turn REPL
cargo run
# > Hello, who are you?
# (streaming response)
# > Tell me more about that
# (streaming response with conversation context)
# > /clear
# History cleared.
# > /quit
```

### Complexity: **High**
Largest phase — establishes the entire project skeleton, Gemini API integration with custom serde, SSE streaming, and basic REPL. ~15-20 files, ~3,000 lines.

---

## Phase 2: Tool System + Filesystem Tools + Tool-Call Loop

**Goal**: The LLM can explore the codebase through Gemini function calling. Ask "What files are here?" and the model autonomously calls `list_directory`, gets results, and responds naturally.

### Files

```
src/
  tool/
    mod.rs             # Tool trait definition
    registry.rs        # ToolRegistry: register, get, execute, to_gemini_tools()
    filesystem.rs      # ReadFileTool, ListDirectoryTool, SearchFilesTool, GrepTool
    shell.rs           # ShellCommandTool with command allowlist
```

### Key Implementation Details

**Tool trait** (`tool/mod.rs`):
```rust
#[async_trait]
pub trait Tool: Send + Sync + Debug {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn declaration(&self) -> FunctionDeclaration;
    async fn execute(&self, args: Value) -> Result<Value>;
    fn available_modes(&self) -> &[Mode];
}
```

**ToolRegistry** (`tool/registry.rs`):
- `register(tool: Box<dyn Tool>)` — adds tool to the registry
- `get(name: &str) -> Option<&dyn Tool>` — lookup by name
- `execute(name: &str, args: Value) -> Result<Value>` — execute by name
- `declarations_for_mode(mode: &Mode) -> Vec<FunctionDeclaration>` — filters by mode
- `to_gemini_tool_definition(mode: &Mode) -> Vec<ToolDefinition>` — generates the tools array for the API request

**Filesystem tools** (`tool/filesystem.rs`):
- `ReadFileTool` — reads file contents, optional `start_line`/`end_line`. Line numbers in output. Large file truncation (100KB cap with warning). Binary file detection
- `ListDirectoryTool` — lists directory contents, optional `recursive` flag. Returns file names, sizes, types. Respects `.gitignore`
- `SearchFilesTool` — glob-based file search (`**/*.rs`). Returns matching paths relative to working dir
- `GrepTool` — regex content search. Returns matches with file path, line number, and context lines. Optional `file_pattern` filter

**Shell tool** (`tool/shell.rs`):
- **Allowlist**: `ls`, `cat`, `head`, `tail`, `find`, `grep`, `rg`, `wc`, `file`, `tree`, `pwd`, `which`, `git`
- Parses command string, validates first token against allowlist
- Executes via `tokio::process::Command` with explicit args (no `sh -c`)
- Captures stdout + stderr, 30-second timeout
- Returns output or error message

**Tool-Call Loop** (extend `gemini/client.rs`):
```
fn run_tool_loop():
  1. Send message to Gemini with tool declarations
  2. If response contains functionCall parts → execute each via ToolRegistry
  3. Build functionResponse parts, append to history
  4. Call Gemini again with updated history
  5. Repeat until text response or max iterations (10)
```

Wire into REPL: when user sends a message, the orchestration goes through `run_tool_loop()` instead of a single API call.

### Dependencies to Add

```toml
glob = "0.3"
regex = "1"
```

### Milestone

```bash
cargo run
# > What files are in the current directory?
# ⠋ Using list_directory...
# There are 5 files: Cargo.toml, src/main.rs, ...

# > Find all Rust files
# ⠋ Using search_files...
# Found 12 .rs files in src/...

# > What does the main function do?
# ⠋ Using read_file on src/main.rs...
# The main function sets up the tokio runtime and...

# > Search for TODO comments in the codebase
# ⠋ Using grep...
# Found 3 TODOs: src/client.rs:42, ...

# > Show me git log
# ⠋ Using shell: git log --oneline -10
# abc123 Initial commit...
```

Shell safety:
```
# > Run rm -rf /
# Error: Command 'rm' is not in the allowlist
```

Tool errors are returned to the model as function responses (not crashes), letting it decide how to handle.

### Complexity: **High**
The tool system is architecturally critical. The tool-call loop is the most important control flow in the application. ~8-10 files, ~2,000 lines.

---

## Phase 3: Sub-Agent Architecture

**Goal**: The orchestrator can spawn Explorer, Planner, and Web Search sub-agents as Gemini function calls. Sub-agents independently research and report back.

### Files

```
src/
  agent/
    mod.rs             # Agent trait
    message.rs         # AgentRequest, AgentResponse, Artifact, ArtifactType
    explorer.rs        # ExplorerAgent (read-only codebase research)
    planner.rs         # PlannerAgent (structured plan creation)
    web_searcher.rs    # WebSearchAgent (Gemini + google_search grounding)
    orchestrator.rs    # Main LLM: conversation history, mode mgmt, sub-agent dispatch
  tool/
    spawn.rs           # SpawnExplorerTool, SpawnPlannerTool, SpawnWebSearchTool
    report.rs          # CreateReportTool (sub-agents structure output)
```

### Key Implementation Details

**Agent trait** (`agent/mod.rs`):
```rust
#[async_trait]
pub trait Agent: Send + Sync {
    fn id(&self) -> &str;
    fn system_prompt(&self) -> &str;
    fn tool_registry(&self) -> &ToolRegistry;
    async fn run(&self, client: &GeminiClient, request: AgentRequest) -> Result<AgentResponse>;
}
```

**Message types** (`agent/message.rs`):
- `AgentRequest` — id (UUID), task (string), context (vec of strings), working_directory
- `AgentResponse` — request_id, agent_id, summary, detailed_report, artifacts vec
- `Artifact` — name, content_type (ArtifactType enum), content
- `ArtifactType` — CodeSnippet { language }, FileContent { path }, DirectoryListing, SearchResults, Plan, Diff

**ExplorerAgent** (`agent/explorer.rs`):
- System prompt: focused on codebase research, finding patterns, understanding architecture
- Own ToolRegistry: read_file, list_dir, search_files, grep, shell, create_report
- `run()`: creates fresh conversation history, runs non-streaming tool-call loop, extracts report from final create_report call
- Max 15 tool-call iterations before forced completion

**PlannerAgent** (`agent/planner.rs`):
- Same tools as Explorer, plan-focused system prompt
- Outputs structured plans: steps, affected files, trade-offs, risk assessment

**WebSearchAgent** (`agent/web_searcher.rs`):
- Uses Gemini with `{"google_search": {}}` native grounding
- No filesystem tools, no tool-call loop (grounding is handled internally by Gemini)
- Takes a research query, returns findings with sources

**Spawn tools** (`tool/spawn.rs`):
- `SpawnExplorerTool` — holds `Arc<GeminiClient>`, creates ExplorerAgent on execute, runs it, returns serialized AgentResponse
- `SpawnPlannerTool` — same pattern, Plan mode only
- `SpawnWebSearchTool` — same pattern, Plan mode only

**CreateReportTool** (`tool/report.rs`):
- Used by sub-agents to structure their output
- Accepts: summary, detailed_report, code_snippets (array of {name, language, content})
- When the sub-agent's tool-call loop sees this tool called, it extracts the report and terminates

**Orchestrator** (`agent/orchestrator.rs`):
- Maintains conversation history (`Vec<Content>`)
- Mode-specific tool registration:
  - **Explore**: filesystem tools + spawn_explorer
  - **Plan**: filesystem tools + spawn_explorer + spawn_planner + spawn_web_search
  - **Execute**: filesystem tools + spawn_explorer + write tools (Phase 4)
- Mode-specific system prompts
- `handle_user_input()`: streaming for main agent, calls run_tool_loop, synthesizes sub-agent reports
- Context window: max 50 turns, prune oldest when exceeded
- Max 30 tool iterations per user turn

### Milestone

```bash
cargo run --mode explore
# > Analyze this project's architecture
# ⠋ Spawning explorer...
# ⠋ Explorer: Using read_file... (internal, not shown to user)
# ⠋ Explorer: Using list_directory...
# ⠋ Explorer: Creating report...
#
# Based on the explorer's analysis, this project has...
# (orchestrator synthesizes explorer's report into natural response)

cargo run --mode plan
# > How should I add caching to the API layer?
# ⠋ Spawning web search: "Rust caching strategies API"...
# ⠋ Spawning planner...
#
# Here's a plan for adding caching:
# 1. Add tower-cache middleware...
# (synthesized from planner + web search reports)
```

### Complexity: **High**
Multi-agent orchestration with mode-dependent tool sets, non-streaming sub-agent loops, report extraction. ~8-10 files, ~2,500 lines.

---

## Phase 4: Execute Mode — Diffs, Approvals, Syntax Highlighting

**Goal**: The model can create and edit files with colorized unified diffs and user approval gates. This is the core "coding assistant" value proposition.

### Files

```
src/
  tool/
    file_write.rs      # WriteFileTool (approval-gated)
    file_edit.rs        # EditFileTool (search/replace, approval-gated)
  ui/
    diff.rs            # Unified diff generation + colorized display
    approval.rs        # ApprovalHandler trait + TerminalApprovalHandler
    markdown.rs        # Markdown rendering for LLM output
```

### Key Implementation Details

**WriteFileTool** (`tool/file_write.rs`):
- Accepts `path` and `content`
- If file exists: reads current contents, generates diff
- If new file: diff shows all lines as additions (`+`)
- Calls `ApprovalHandler::request_approval(path, old, new)`
- On approval: creates parent directories, writes file, returns `{"status": "applied"}`
- On rejection: returns `{"status": "rejected", "reason": "User declined the change"}`
- Execute mode only (`available_modes() → [Execute]`)

**EditFileTool** (`tool/file_edit.rs`):
- Accepts `path`, `old_text`, `new_text` (search/replace)
- Reads file, finds `old_text` exact match
- Generates new content with replacement, calls ApprovalHandler
- Error cases: file not found, old_text not found, multiple occurrences (applies first match, warns)

**Diff display** (`ui/diff.rs`):
- `similar` crate (Myers diff algorithm) generates unified diff
- Formats with standard headers: `--- a/{path}` / `+++ b/{path}`
- Hunk headers: `@@ -start,count +start,count @@`
- ANSI coloring via crossterm:
  - `Red` for deleted lines (`-`)
  - `Green` for added lines (`+`)
  - `DarkGrey` for context lines
  - `Cyan` for hunk headers (`@@`)
- Change summary: `"2 additions, 1 deletion"`
- File path and summary displayed above diff

**ApprovalHandler** (`ui/approval.rs`):
```rust
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request_approval(&self, file_path: &str, old: &str, new: &str) -> Result<bool>;
}
```
- `TerminalApprovalHandler`: displays colorized diff, prompts `Apply this change? [y/N]` via `dialoguer::Confirm` with **default No**
- `MockApprovalHandler`: configurable auto-approve/reject for testing

**Markdown rendering** (`ui/markdown.rs`):
- Headers → bold + color
- Code blocks → syntax highlighted via `syntect` (detects language from fence tag)
- Inline code → dimmed background
- Lists → bullet/number prefixes
- Bold, italic → ANSI escape sequences
- Links → `text (url)` format

### Dependencies to Add

```toml
similar = "2"
syntect = "5"
dialoguer = "0.11"
```

### Milestone

```bash
cargo run --mode execute
# > Create a new file called hello.rs with a hello world program
#
# --- /dev/null
# +++ b/hello.rs
# @@ -0,0 +1,3 @@
# +fn main() {
# +    println!("Hello, world!");
# +}
#
#   File: hello.rs
#   Changes: 3 additions, 0 deletions
#
# Apply this change? [y/N] y
# ✓ File created: hello.rs

# > Add a goodbye function to hello.rs
#
# --- a/hello.rs
# +++ b/hello.rs
# @@ -1,3 +1,7 @@
#  fn main() {
#      println!("Hello, world!");
# +    goodbye();
# +}
# +
# +fn goodbye() {
# +    println!("Goodbye!");
#  }
#
#   File: hello.rs
#   Changes: 4 additions, 0 deletions
#
# Apply this change? [y/N] n
# Change rejected. I'll try a different approach...
```

### Complexity: **High**
Diff generation, syntax highlighting, approval flow, and markdown rendering. ~6-8 files, ~2,000 lines.

---

## Phase 5: Configuration System + Enhanced REPL

**Goal**: TOML-based configuration (user + project level), approval policies, personality settings, enhanced slash commands, token tracking, shell prefix, multiline input, and rate limit handling.

### Files

```
src/
  config.rs            # Rewrite: layered TOML config + CLI + env
```

### Key Implementation Details

**Layered configuration** (extend `config.rs`):
- Resolution order: CLI flags > project `.closed-code/config.toml` > user `~/.closed-code/config.toml` > defaults
- Config struct:
  ```rust
  pub struct Config {
      pub api_key: String,
      pub model: String,
      pub default_mode: Mode,
      pub working_directory: PathBuf,
      pub approval_policy: ApprovalPolicy,
      pub personality: Personality,
      pub shell_allowlist: Vec<String>,
      pub max_tool_iterations: usize,
      pub context_window_turns: usize,
      pub verbose: bool,
  }
  ```
- Example `config.toml`:
  ```toml
  model = "gemini-3.1-pro-preview"
  approval_policy = "suggest"
  personality = "pragmatic"
  default_mode = "explore"
  context_window_turns = 50

  [shell]
  additional_allowlist = ["docker", "cargo", "npm"]
  ```

**Approval policies**:
- `Suggest` (default) — prompts for file writes AND shell commands
- `AutoEdit` — auto-approves file operations, prompts for shell commands
- `FullAuto` — fully autonomous, no prompts at all

**Personality system**:
- `friendly` — warm, encouraging, casual language
- `pragmatic` — direct, concise, code-focused
- `none` — minimal personality, just answers
- Modifies the system prompt prefix

**Enhanced REPL** (extend `repl.rs`):
- New slash commands:
  | Command | Description |
  |---------|-------------|
  | `/model [name]` | Show or switch model |
  | `/permissions [policy]` | Show or change approval policy |
  | `/personality [style]` | Show or change personality |
  | `/status` | Token usage, model, mode, policy, session info |
  | `/explore` | Switch to Explore mode |
  | `/plan` | Switch to Plan mode |
  | `/execute` | Switch to Execute mode |
  | `/mode` | Show current mode |
  | `/clear` | Clear conversation history |
  | `/help` | Show all commands |
  | `/quit` | Exit |
- `!` prefix for local shell commands: `!ls -la` executes locally and displays output (not sent to LLM)
- `Ctrl+G` opens `$EDITOR` (or `vi` fallback) for composing longer prompts
- Tab completion for slash commands

**Token usage tracking**:
- Accumulate `UsageMetadata` from every API response
- Track: total prompt tokens, total completion tokens, total tokens
- `/status` displays current usage
- Warn when approaching context window limits

**Rate limit handling** (enhance `gemini/client.rs`):
- Parse `Retry-After` header from 429 responses
- Exponential backoff with jitter
- Display `"Rate limited, retrying in Xs..."` to user with countdown
- Max 5 retries for rate limits (separate from the 3-retry server error limit)

**Context window management**:
- Configurable max turns (default 50)
- When exceeding: prune oldest turns, keep system prompt + most recent N turns
- Display notification: `"Context pruned: removed 10 oldest turns"`

### Dependencies to Add

```toml
toml = "0.8"
```

### Milestone

```bash
# Config file respected
echo 'personality = "friendly"' > ~/.closed-code/config.toml
cargo run
# > Hi!
# Hey there! 👋 Great to see you! What can I help with today?

# Token tracking
# > /status
# Mode: explore | Model: gemini-3.1-pro-preview | Policy: suggest
# Tokens: 1,234 prompt + 567 completion = 1,801 total

# Approval policy change
# > /permissions auto_edit
# Approval policy changed to: auto_edit

# Shell prefix
# > !git status
# On branch main
# Changes not staged for commit: ...

# Multiline input
# > Ctrl+G
# (opens $EDITOR for composing a long prompt)

# Rate limit handling
# > (after many requests)
# Rate limited. Retrying in 3s...
# Rate limited. Retrying in 7s...
# (response arrives)
```

### Complexity: **Medium-High**
Extending existing systems rather than building new ones. ~5-8 files modified, ~1,500 lines.

---

## Phase 6: Git Integration + Diff Review

**Goal**: Deep git awareness — branch info in context, `/diff` for reviewing changes, `/review` for LLM code review, `/commit` for auto-commit with generated messages.

### Files

```
src/
  git/
    mod.rs             # Module re-exports
    context.rs         # GitContext: branch, changes, merge-base detection
    diff.rs            # Git diff generation (unstaged, staged, branch)
    commit.rs          # Auto-commit with LLM-generated messages
```

### Key Implementation Details

**GitContext** (`git/context.rs`):
```rust
pub struct GitContext {
    pub is_git_repo: bool,
    pub current_branch: Option<String>,
    pub default_branch: Option<String>,
    pub merge_base: Option<String>,
    pub has_uncommitted_changes: bool,
    pub changed_files: Vec<ChangedFile>,
}

pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,  // Added, Modified, Deleted, Renamed
}
```
- Detection on startup: `git rev-parse --is-inside-work-tree`
- Branch: `git branch --show-current`
- Default branch: checks for `main` then `master`
- Merge base: `git merge-base HEAD <default-branch>`
- Changed files: `git diff --name-status` + `git diff --cached --name-status`

**Git diff** (`git/diff.rs`):
- `git_diff_unstaged()` → raw diff string
- `git_diff_staged()` → staged changes only
- `git_diff_branch(base)` → changes since branching
- All return raw diff text, displayed through `ui/diff.rs` colorizer

**Auto-commit** (`git/commit.rs`):
- `generate_commit_message(diff: &str, client: &GeminiClient)` → sends diff to LLM with prompt "Generate a concise commit message for these changes"
- `auto_commit(message: &str)` → `git add -A && git commit -m "message"`
- `auto_commit_files(files: &[&str], message: &str)` → stages specific files
- Approval gate: displays proposed commit message, `"Commit with this message? [y/N]"`

**New slash commands** (extend `repl.rs`):

| Command | Description |
|---------|-------------|
| `/diff` | Show all uncommitted changes (colorized) |
| `/diff staged` | Show only staged changes |
| `/diff branch` | Show changes since branching from default branch |
| `/review` | Send uncommitted changes to LLM for code review |
| `/review HEAD~N` | Review specific commits |
| `/commit` | Generate commit message via LLM, commit with approval |

**Git-aware system prompt**:
- When in a git repo, append to system prompt:
  ```
  Git context: On branch `feature/auth`, 3 uncommitted changes.
  Changed files: src/auth.rs (modified), src/lib.rs (modified), tests/auth_test.rs (added)
  Recent commits: abc123 Add login endpoint, def456 Setup database...
  ```

**Protected paths** (extend write tools):
- `.git/` is always read-only, even in FullAuto mode
- `.closed-code/` is read-only
- Write tools check target path: `if path.starts_with(".git") { return Err(ProtectedPath) }`

### Milestone

```bash
# Git-aware prompt
cargo run
# closed-code (explore) [main, 3 changes] >

# Diff review
# > /diff
# --- a/src/main.rs
# +++ b/src/main.rs
# @@ -10,3 +10,5 @@
# ...colorized diff...

# LLM code review
# > /review
# ⠋ Reviewing changes...
# I've reviewed your uncommitted changes. Here's my feedback:
# 1. src/auth.rs:42 — The password hash comparison is timing-safe, good.
# 2. src/lib.rs:15 — Consider adding error handling for the database connection.
# ...

# Auto-commit
# > /commit
# ⠋ Generating commit message...
# Proposed commit message: "Add authentication endpoint with password hashing"
# Commit with this message? [y/N] y
# ✓ Committed: abc1234

# Protected paths
# > (in execute mode) Write to .git/config
# Error: Cannot modify protected path: .git/config
```

### Complexity: **Medium**
Mostly shell-outs to git commands with output parsing. ~4-6 files, ~1,200 lines.

---

## Phase 7: Sandboxing + Security Hardening

**Goal**: Platform-specific sandboxing for shell commands. macOS uses Seatbelt, Linux uses Landlock + seccomp. Multiple sandbox modes with protected path enforcement.

### Files

```
src/
  sandbox/
    mod.rs             # Sandbox trait, SandboxMode enum, platform dispatch
    macos.rs           # Seatbelt sandbox-exec integration
    linux.rs           # Landlock + seccomp integration
    fallback.rs        # Unsandboxed fallback with warning
```

### Key Implementation Details

**Sandbox abstraction** (`sandbox/mod.rs`):
```rust
pub enum SandboxMode {
    ReadOnly,         // No writes anywhere, no network
    WorkspaceWrite,   // Writes only within workspace dir, no network
    FullAccess,       // No restrictions (requires explicit opt-in)
}

pub trait Sandbox: Send + Sync {
    fn execute_command(&self, command: &str, args: &[&str], cwd: &Path) -> Result<Output>;
    fn mode(&self) -> SandboxMode;
}
```

**macOS Seatbelt** (`sandbox/macos.rs`):
- Generates Seatbelt profile strings dynamically based on `SandboxMode`:
  ```scheme
  ;; WorkspaceWrite profile
  (version 1)
  (deny default)
  (allow process-exec)
  (allow file-read*)
  (allow file-write* (subpath "/path/to/workspace"))
  (deny network*)
  ```
- Executes commands via: `sandbox-exec -p <profile> /path/to/command arg1 arg2`
- Validates `sandbox-exec` availability on startup
- ReadOnly: deny file-write*, deny network*
- WorkspaceWrite: allow file-write* in workspace subpath, deny network*
- FullAccess: allow default (no Seatbelt)

**Linux Landlock** (`sandbox/linux.rs`):
- Creates Landlock ruleset based on SandboxMode
- ReadOnly: `LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR`
- WorkspaceWrite: adds `LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_MAKE_REG` for workspace path
- Applies via `prctl(PR_SET_NO_NEW_PRIVS, 1)` + `landlock_restrict_self()`
- Optional seccomp BPF filter for additional syscall restriction

**Fallback** (`sandbox/fallback.rs`):
- For unsupported platforms (Windows, etc.)
- Logs a tracing warning: "Sandboxing not available on this platform"
- Executes commands directly via existing shell tool path

**Integration** (modify `tool/shell.rs`):
- All shell command execution routed through the `Sandbox` trait
- Sandbox mode selected from config (`--sandbox` flag or `config.toml`)
- `/sandbox` slash command to view/change mode

**Protected paths enforcement** (extend write tools):
- Hardcoded: `.git/`, `.closed-code/`, `.env`, `*.pem`, `*.key`
- Configurable additions in `config.toml`:
  ```toml
  [security]
  protected_paths = [".secrets/", "credentials.json"]
  ```
- Write tools check path before calling ApprovalHandler
- Error: `"Cannot modify protected path: .env"`

### Dependencies to Add

```toml
# Platform-specific
[target.'cfg(target_os = "linux")'.dependencies]
landlock = "0.4"
seccompiler = "0.4"
# No additional deps for macOS (uses system sandbox-exec)
```

### Milestone

```bash
# macOS sandbox
cargo run --sandbox workspace-write
# > (LLM calls shell: git log)
# ✓ Sandboxed: git log runs successfully (read-only operation)

# > (LLM tries to write outside workspace via shell)
# ✗ Sandbox denied: write access to /etc/hosts

# Linux sandbox
cargo run --sandbox read-only
# > (LLM tries to write any file)
# ✗ Sandbox denied: write operations not permitted in read-only mode

# Protected paths
# > (in full-access mode) Edit .env
# Error: Cannot modify protected path: .env

# Check mode
# > /sandbox
# Sandbox mode: workspace-write (macOS Seatbelt)

# Fallback
# (on unsupported platform)
# Warning: Sandboxing not available. Running without sandbox.
```

### Complexity: **High**
Platform-specific code, Seatbelt profile generation, Landlock FFI. Requires testing on both platforms. ~5-7 files, ~2,000 lines.

---

## Phase 8: Session Management + MCP + Advanced Features

**Goal**: Session persistence (resume, fork, compact), MCP client for external tool servers, image input, transcript logging.

### Files

```
src/
  session/
    mod.rs             # Session struct, SessionStore
    persistence.rs     # JSONL read/write
    operations.rs      # Resume, fork, compact, new
  mcp/
    mod.rs             # MCP client module
    client.rs          # McpClient: STDIO transport, tool discovery, execution
    config.rs          # MCP server configuration parsing
```

### Key Implementation Details

**Session persistence** (`session/`):
- `Session` struct: thread_id (UUID), conversation history, metadata (model, mode, start_time, token_usage)
- Storage: `~/.closed-code/sessions/<thread_id>.jsonl`
- Events logged (one JSON per line):
  ```jsonl
  {"type":"user_message","content":"Hello","timestamp":"2026-02-21T10:00:00Z"}
  {"type":"assistant_message","content":"Hi there!","timestamp":"2026-02-21T10:00:01Z"}
  {"type":"tool_call","name":"read_file","args":{"path":"src/main.rs"},"timestamp":"..."}
  {"type":"tool_response","name":"read_file","result":"...","timestamp":"..."}
  {"type":"mode_change","from":"explore","to":"plan","timestamp":"..."}
  ```

**Session operations**:
- **Resume** (`closed-code resume` or `/resume`):
  - Lists recent sessions: `[1] 2h ago — "Explain the auth flow" [2] 1d ago — "Add caching to API" ...`
  - Select session → replay JSONL to reconstruct `Vec<Content>` + metadata
  - Continue conversation with full context
- **Fork** (`/fork`):
  - Copy current session JSONL to new UUID
  - Continue in fork, original preserved
  - Print: `"Forked session → new thread: abc-123"`
- **Compact** (`/compact`):
  - Send conversation to LLM: `"Summarize this conversation in 500 words, preserving key decisions and context"`
  - Replace conversation history: system prompt + summary + last 5 turns
  - Print: `"Compacted: 47 turns → 6 turns, freed ~12K tokens"`
- **New** (`/new`):
  - Archive current session (write final JSONL event)
  - Start fresh with new UUID

**MCP client** (`mcp/`):
- Config in `config.toml`:
  ```toml
  [mcp_servers.filesystem]
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-filesystem", "/path"]
  transport = "stdio"
  enabled = true

  [mcp_servers.database]
  command = "mcp-server-postgres"
  args = ["postgresql://localhost/mydb"]
  transport = "stdio"
  enabled = true
  ```
- `McpClient`:
  - Launches MCP server as a child process on session start
  - STDIO transport: sends JSON-RPC over stdin, reads from stdout
  - Tool discovery: calls `tools/list` method → gets available tools
  - Tool namespacing: `filesystem/read_file`, `database/query` to avoid conflicts with built-in tools
  - Registers discovered tools into ToolRegistry (with MCP-specific executor that routes to the child process)
  - Health check: periodic ping, reconnect on failure
  - Graceful shutdown: sends `shutdown` + `exit` on session end, kills child process

**Image input**:
- CLI: `closed-code ask --image screenshot.png "Implement this UI"`
- REPL: `/image path/to/screenshot.png` followed by prompt
- Reads image file, detects MIME type (PNG, JPEG, GIF, WEBP)
- Base64-encodes and sends as `InlineData` part in Gemini Content:
  ```rust
  Part::InlineData { mime_type: "image/png", data: base64_string }
  ```

**Transcript logging**:
- Writes full action transcript to `~/.closed-code/transcripts/<timestamp>.md`
- Contains: user messages, LLM responses, tool calls/results, diffs, approvals
- Configurable: `transcript_logging = true` in config.toml
- Markdown format for human readability

**Additional slash commands**:

| Command | Description |
|---------|-------------|
| `/resume` | Resume a previous session |
| `/fork` | Fork current session into new thread |
| `/compact` | Summarize conversation to free tokens |
| `/new` | Start fresh conversation |
| `/agent` | Show sub-agent activity log |
| `/experimental [feature]` | Toggle experimental features |
| `/history` | Show conversation history summary |
| `/export [file]` | Export conversation to markdown file |
| `/image <path>` | Attach an image to the next message |

### Dependencies to Add

```toml
base64 = "0.22"
chrono = "0.4"
```

### Milestone

```bash
# Session resume
cargo run
# > Explain the auth flow
# (conversation happens)
# > /quit

cargo run resume
# [1] 2m ago — "Explain the auth flow" (explore, 1.2K tokens)
# [2] 1d ago — "Add caching" (plan, 5.4K tokens)
# Select session: 1
# Resumed session. Last message: "The auth flow uses JWT..."
# > Tell me more about the token refresh

# Fork
# > /fork
# Forked → new session abc-123. Original preserved.

# Compact
# > /compact
# Compacted: 47 turns → 6 turns, freed ~12,000 tokens

# MCP
# (with filesystem MCP server configured)
cargo run
# MCP: Connected to 'filesystem' (3 tools)
# > Use the filesystem server to read /etc/hosts
# ⠋ Using filesystem/read_file...
# The /etc/hosts file contains...

# Image input
cargo run -- ask --image wireframe.png "Implement this UI component"
# ⠋ Analyzing image...
# I can see a login form with...
```

### Complexity: **High**
Session persistence, MCP JSON-RPC protocol, image handling. ~8-12 files, ~2,500 lines.

---

## Phase 9: Full-Screen TUI with Ratatui

**Goal**: Replace the line-based REPL with a full-screen terminal UI. Three-layer architecture: App (event loop), ChatWidget (scrollable conversation), BottomPane (input + status). Alternate screen mode, viewport scrolling, inline approval overlays, streaming character-by-character.

### Files

```
src/
  tui/
    mod.rs                 # TUI entry point, exports
    app.rs                 # App struct: event loop, state machine, terminal setup
    chat_widget.rs         # Scrollable conversation with message bubbles
    bottom_pane.rs         # Input area + status bar
    approval_overlay.rs    # Modal diff review overlay
    diff_view.rs           # Full-screen diff viewer (vim keys)
    input_handler.rs       # Keyboard input processing
    markdown_widget.rs     # Ratatui-native markdown rendering
    spinner_widget.rs      # Animated spinners
```

### Key Implementation Details

**App** (`tui/app.rs`):
- Sets up `Terminal<CrosstermBackend<Stdout>>`
- Enters alternate screen mode + raw mode on start
- Restores terminal on exit (including on panic via `std::panic::set_hook`)
- Event loop processes two async channels:
  1. Terminal events (keyboard, mouse, resize) via `crossterm::event::EventStream`
  2. App events (API response chunks, tool results, sub-agent status) via `tokio::sync::mpsc`
- State machine:
  ```rust
  enum AppState {
      Idle,               // Waiting for user input
      Thinking,           // Spinner shown, waiting for first token
      Streaming,          // Tokens arriving, rendering in real-time
      AwaitingApproval,   // Diff overlay shown, waiting for y/n
      DiffView,           // Full-screen diff viewer active
      ShellRunning,       // Shell command executing with output
  }
  ```
- Tick rate: 100ms for smooth animations

**ChatWidget** (`tui/chat_widget.rs`):
- Renders conversation as a scrollable list of message cells
- **User messages**: styled with user color, right-border indicator or `> ` prefix
- **Assistant messages**: full-width, markdown rendered
- **Tool calls**: compact indicator: `"⠋ Using read_file on src/main.rs..."`
- **Sub-agent activity**: `"⠋ Explorer is researching (3 tool calls)..."`
- **Streaming**: tokens appear character-by-character as they arrive from the SSE stream
- **Viewport scrolling**:
  - Arrow Up/Down or mouse wheel to scroll through history
  - Auto-scroll to bottom on new content (unless user has scrolled up)
  - Scroll position indicator (e.g., `"↑ 42 more lines"`)
  - Page Up/Page Down for fast scrolling

**BottomPane** (`tui/bottom_pane.rs`):
- **Input area**: multi-line text input with cursor, line editing
  - Uses `tui-textarea` for rich text input
  - Enter submits, Shift+Enter for newline (or configurable)
- **Slash command autocomplete**: typing `/` shows a dropdown with matching commands
- **Status bar** (bottom line):
  ```
  explore | gemini-3.1-pro-preview | suggest | 1,234 tokens | [main, 2 changes]
  ```
  - Mode (colored), model, approval policy, token count, git info
- **Input hint**: dim text when input is empty: `"Type a message, / for commands, ! for shell, @ for files"`

**Approval overlay** (`tui/approval_overlay.rs`):
- Triggered when a write tool needs approval
- Modal overlay on top of the chat:
  - Title: `"Proposed change to src/main.rs"`
  - Scrollable colorized diff (reuses `ui/diff.rs`)
  - Change summary: `"4 additions, 1 deletion"`
  - Actions: `[y] Apply  [n] Reject  [d] Full diff view`
- Arrow keys scroll through large diffs within the overlay
- After approval/rejection, overlay closes, chat continues

**Diff view** (`tui/diff_view.rs`):
- Full-screen diff viewer (activated with `d` from approval overlay)
- Side-by-side or unified display (toggle with `t`)
- Syntax highlighting via syntect
- Line number gutters
- Vim-style navigation: `j`/`k` for line, `Ctrl+d`/`Ctrl+u` for half-page, `gg`/`G` for top/bottom
- `q` returns to approval overlay

**Keyboard handling** (`tui/input_handler.rs`):

| Key | Action |
|-----|--------|
| Enter | Submit input |
| Ctrl+C | Cancel current operation / clear input |
| Ctrl+D | Exit application |
| Ctrl+G | Open $EDITOR for multiline input |
| Ctrl+L | Clear and redraw screen |
| Ctrl+R | Session resume picker |
| Page Up/Down | Scroll conversation |
| Arrow Up/Down | Scroll (when input empty) / input history |
| Tab | Autocomplete slash commands |
| Escape | Dismiss overlays |
| `y`/`n`/`d` | Approval actions (when overlay active) |

**Markdown widget** (`tui/markdown_widget.rs`):
- Parses markdown into ratatui `Spans` and `Line` sequences
- **Headers**: bold text with accent color, underline for H1
- **Code blocks**: background color fill, syntax highlighting (syntect → ratatui Style mapping), language label
- **Inline code**: subtle background color change
- **Lists**: bullet `•` and numbered prefixes with indentation
- **Bold**: `Style::default().bold()`
- **Italic**: `Style::default().italic()`
- **Links**: underlined text with URL in parentheses

**Spinner widget** (`tui/spinner_widget.rs`):
- Animated braille pattern spinner: `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏`
- Dot pattern for streaming: `⠁ ⠂ ⠄ ⡀ ⢀ ⠠ ⠐ ⠈`
- Updates every 100ms tick
- Displays alongside status text: `"⠋ Thinking..."`, `"⠋ Using grep..."`

**Integration with existing systems**:
- `TuiApprovalHandler` replaces `TerminalApprovalHandler`: triggers the approval overlay instead of inline prompt
- Streaming: GeminiClient sends tokens via `mpsc::Sender<AppEvent>`, chat widget renders them
- All slash commands adapted to work in TUI context (output rendered in chat area)
- Status bar updates reactively on mode/model/policy changes

### Dependencies to Add

```toml
ratatui = "0.29"
tui-textarea = "0.7"
```

### Milestone

```
┌─ closed-code ──────────────────────────────────────────────────────┐
│                                                                     │
│  > What files are in this project?                                  │
│                                                                     │
│  ⠋ Using list_directory...                                          │
│                                                                     │
│  Here are the files in your project:                                │
│                                                                     │
│  ```                                                                │
│  Cargo.toml                                                         │
│  src/                                                               │
│    main.rs                                                          │
│    lib.rs                                                           │
│  ```                                                                │
│                                                                     │
│  The project has 2 source files and a Cargo.toml manifest.          │
│                                                                     │
│  > Create a README.md with project description                      │
│                                                                     │
│  ┌─ Proposed change to README.md ──────────────────────────────┐    │
│  │ +++ b/README.md                                              │    │
│  │ @@ -0,0 +1,5 @@                                             │    │
│  │ +# My Project                                                │    │
│  │ +                                                             │    │
│  │ +A Rust application for...                                    │    │
│  │                                                               │    │
│  │ 5 additions, 0 deletions                                     │    │
│  │ [y] Apply  [n] Reject  [d] Full diff                         │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                                                     │
├─────────────────────────────────────────────────────────────────────┤
│ Type a message, / for commands, ! for shell, @ for files            │
│ █                                                                    │
├─────────────────────────────────────────────────────────────────────┤
│ explore │ gemini-3.1-pro-preview │ suggest │ 1,234 tok │ main       │
└─────────────────────────────────────────────────────────────────────┘
```

Specific verifications:
- Full-screen TUI launches in alternate screen mode
- Streaming responses appear character-by-character
- Scrolling through conversation history works (arrow keys, mouse wheel, Page Up/Down)
- Approval overlay is modal: diff is scrollable, y/n/d keys work
- Full-screen diff viewer has syntax highlighting and vim navigation
- Status bar is always visible and accurate
- Ctrl+C cancels, Ctrl+D exits, terminal is always restored cleanly
- Terminal resize reflows the layout correctly

### Complexity: **Very High**
The largest single phase. Full-screen TUI with ratatui is a major UI engineering effort. ~10-15 files, ~3,500 lines.

---

## Phase 10: Polish, Animations, Multi-Agent, Feature Parity

**Goal**: The "wow factor" phase. Fuzzy file search, multi-agent parallel execution with git worktrees, desktop notifications, smooth animations, headless mode, and everything needed for full Codex CLI parity.

### Scope

**1. Fuzzy file search (`@` trigger)**

```
src/tui/file_picker.rs   # Fuzzy file search overlay
```

- Type `@` in the input to open a fuzzy file search overlay
- Builds file index from workspace (recursive, respecting `.gitignore`)
- Fuzzy matching using `nucleo` crate (same as used by helix editor)
- Overlay shows top 10 matches, updates as you type
- Arrow keys to navigate, Tab/Enter to insert selected path into input
- Escape to dismiss
- Caches file index, refreshes on `/refresh` or after file modifications

**2. Multi-agent parallel execution**

```
src/agent/parallel.rs     # Multi-agent coordinator
```

- `/agent spawn "Add unit tests for auth module"` — creates an independent agent working on a task
- Each agent gets an isolated git worktree:
  ```
  git worktree add .closed-code/worktrees/<agent-id> -b agent/<agent-id>
  ```
- Agents run concurrently with their own conversation state, tool registries, and working directories
- `/agent status` — shows all running agents:
  ```
  #1 [running] "Add unit tests" (12 tool calls, 45s)
  #2 [done]    "Fix linting errors" (8 tool calls, 22s)
  ```
- `/agent join <id>` — merges an agent's worktree back to current branch:
  ```
  Merging agent #2 worktree...
  ✓ Merged 3 files from agent/abc-123 → main
  ```
- `/agent kill <id>` — terminate a running agent, clean up worktree
- Cleanup stale worktrees on startup

**3. Desktop notifications**

```
src/ui/notify.rs          # Cross-platform desktop notifications
```

- Triggers:
  - Long-running agent completes (>30s)
  - Approval needed and terminal not focused
  - Session error (rate limit, API failure)
- macOS: `osascript -e 'display notification "Agent completed" with title "closed-code"'`
- Linux: `notify-send "closed-code" "Agent completed"`
- Configurable: `notifications = true` in config.toml
- Check terminal focus via `crossterm::event::FocusGained`/`FocusLost`

**4. Rich TUI animations**

- **Typing indicator**: Animated dots when LLM is generating: `Thinking...` → `Thinking....` → `Thinking.....`
- **Message entrance**: New messages fade in with a brief 200ms opacity transition (simulated with dim → normal color)
- **Diff highlight flash**: When approval overlay opens, changed lines briefly flash bright before settling to normal diff colors
- **Status bar transitions**: Smooth color pulse on mode switch (old mode color → new mode color over 300ms)
- **Progress bar for sub-agents**: Approximate progress based on tool call count vs typical max: `Explorer ████░░░░░░ 40%`
- **Scroll momentum**: Smooth scrolling with deceleration (not jump-to-line)
- All animations respect `reduce_motion = true` config option

**5. Headless execution mode**

- `closed-code exec "Fix all clippy warnings"` — runs without TUI, outputs results to stdout
- No interactive prompts in headless mode (uses FullAuto approval policy)
- Exit code: 0 on success, 1 on failure
- JSON output: `--output json` for machine-readable results
- Pipe-friendly: detects non-TTY and auto-switches to headless

**6. Error recovery and edge cases**

- **Network disconnect mid-stream**: Detect broken connection, display "Connection lost. Retrying...", attempt reconnect with exponential backoff
- **Large file handling**: Files >1MB get truncated with warning before sending to LLM. Binary files detected and skipped with summary
- **Circular tool-call detection**: If same tool called with identical args 3 times in a row, break the loop with error to model
- **Max conversation length**: Warning at 80% of context window, auto-compact suggestion
- **Concurrent read tools**: Multiple read-only tools called in the same turn execute in parallel (`tokio::join!`)

**7. Configuration completions**

- `.closed-code-ignore` file for excluding files from indexing (like `.gitignore` syntax)
- `CLOSED_CODE_HOME` env var to override config directory
- `--profile <name>` flag for named config profiles:
  ```toml
  [profiles.review]
  approval_policy = "suggest"
  default_mode = "explore"

  [profiles.hack]
  approval_policy = "full_auto"
  default_mode = "execute"
  ```

**8. Testing infrastructure**

```
tests/
  integration/
    mod.rs
    tool_loop_test.rs     # Tool-call loop integration tests
    approval_test.rs      # Approval flow with MockApprovalHandler
    session_test.rs       # Session persistence round-trip
    diff_test.rs          # Diff output snapshot tests
  fixtures/
    recorded_responses/   # Recorded API responses for replay
```

- `MockGeminiClient` — returns pre-recorded responses for deterministic testing
- `MockApprovalHandler` — configurable auto-approve/reject/delay
- Replay harness: record API interactions, replay for regression testing
- Snapshot tests: verify diff output formatting matches expected output
- CI-friendly: all tests run headless (no TUI, no API calls)

### Dependencies to Add

```toml
nucleo = "0.5"
```

### Milestone

```bash
# Fuzzy file search
cargo run
# > @
# ┌─ Files ────────────────┐
# │ > src/main.rs           │
# │   src/gemini/client.rs  │
# │   src/tool/registry.rs  │
# │   Cargo.toml            │
# └─────────────────────────┘
# (type to filter, arrows to navigate, Enter to insert)

# Multi-agent
# > /agent spawn "Add comprehensive tests for the auth module"
# Agent #1 spawned in worktree .closed-code/worktrees/agent-abc
#
# > /agent spawn "Fix all clippy warnings"
# Agent #2 spawned in worktree .closed-code/worktrees/agent-def
#
# > /agent status
# #1 [running] "Add comprehensive tests" (8 tool calls, 1m 12s)
# #2 [done]    "Fix clippy warnings" (5 tool calls, 34s)
#
# > /agent join 2
# Merging agent #2...
# ✓ Merged 6 files from agent/def → main

# Headless mode
closed-code exec "Create a basic REST API with three endpoints" --output json
# {"status":"success","files_created":["src/routes.rs","src/handlers.rs"],...}

# Desktop notification
# (after long agent completes while terminal is unfocused)
# macOS notification: "closed-code — Agent #1 completed: Add comprehensive tests"

# Animations
# (streaming response with smooth typing indicator)
# (approval overlay opens with diff lines flashing briefly)
# (status bar pulses when switching modes)

# Tests
cargo test
# running 24 tests
# test tool_loop ... ok
# test approval_flow ... ok
# test session_roundtrip ... ok
# ...
# test result: ok. 24 passed; 0 failed
```

### Complexity: **Very High**
Catch-all phase with many independent features. Multi-agent with git worktrees is the most complex piece. ~10-15 files, ~3,000 lines.

---

## Codex CLI Feature Parity Checklist

Features mapped to phases:

| Codex CLI Feature | Phase | Status After Phase |
|-------------------|-------|-------------------|
| Streaming conversation | 1 | ✓ |
| Multi-turn context | 1 | ✓ |
| Codebase exploration via tools | 2 | ✓ |
| Function calling / tool-call loop | 2 | ✓ |
| Shell command execution (safe) | 2 | ✓ |
| Sub-agent delegation | 3 | ✓ |
| Web search (grounded) | 3 | ✓ |
| File creation with diff review | 4 | ✓ |
| File editing with diff review | 4 | ✓ |
| Syntax-highlighted diffs | 4 | ✓ |
| Markdown rendering | 4 | ✓ |
| User approval gate (default No) | 4 | ✓ |
| TOML configuration (user + project) | 5 | ✓ |
| Multiple approval policies | 5 | ✓ |
| Personality settings | 5 | ✓ |
| Token usage tracking (/status) | 5 | ✓ |
| Shell prefix (`!`) | 5 | ✓ |
| Multiline input (Ctrl+G) | 5 | ✓ |
| Rate limit retry with backoff | 5 | ✓ |
| Context window management | 5 | ✓ |
| Comprehensive slash commands (25+) | 5, 6, 8 | ✓ |
| Git branch awareness | 6 | ✓ |
| /diff (view changes) | 6 | ✓ |
| /review (LLM code review) | 6 | ✓ |
| /commit (auto-commit) | 6 | ✓ |
| Protected paths | 6 | ✓ |
| macOS Seatbelt sandbox | 7 | ✓ |
| Linux Landlock sandbox | 7 | ✓ |
| Sandbox modes (read-only, workspace-write, full-access) | 7 | ✓ |
| Session resume | 8 | ✓ |
| Session fork | 8 | ✓ |
| Conversation compaction | 8 | ✓ |
| MCP tool servers | 8 | ✓ |
| Image input | 8 | ✓ |
| Transcript logging | 8 | ✓ |
| Full-screen TUI | 9 | ✓ |
| Scrollable conversation | 9 | ✓ |
| Inline approval overlays | 9 | ✓ |
| Full-screen diff viewer | 9 | ✓ |
| Status bar | 9 | ✓ |
| Slash command autocomplete | 9 | ✓ |
| Character-by-character streaming | 9 | ✓ |
| Animated spinners | 9 | ✓ |
| @ fuzzy file search | 10 | ✓ |
| Multi-agent parallel execution | 10 | ✓ |
| Git worktrees for agents | 10 | ✓ |
| Desktop notifications | 10 | ✓ |
| Smooth animations | 10 | ✓ |
| Headless exec mode | 10 | ✓ |
| Config profiles | 10 | ✓ |
| Testing infrastructure | 10 | ✓ |

---

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| Single binary, no Cargo workspace | Components are tightly coupled; workspace adds build complexity |
| Line-based REPL first, TUI last (Phase 9) | Validates all business logic before investing in complex UI |
| Sandbox in Phase 7, not Phase 1 | Approval gates (Phase 4) provide safety before sandbox hardening |
| MCP alongside sessions (Phase 8) | Both need config infra (Phase 5) and process lifecycle management |
| Config system in Phase 5, not Phase 1 | Env vars + CLI flags work fine until core functionality is proven |
| Git worktrees in Phase 10, not Phase 6 | Advanced feature requiring both git and parallel agent infrastructure |
| ratatui over custom terminal rendering | Battle-tested, active ecosystem, familiar widget model |
| nucleo for fuzzy matching | Used by helix editor, high performance, good Unicode support |
| JSONL for session persistence | Append-only, easy to parse incrementally, human-readable |
| Phases 6/7/8 parallelizable | Largely independent after Phase 5; allows concurrent development |

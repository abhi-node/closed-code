# Phase 5: Configuration System + Enhanced REPL

**Goal**: TOML-based layered configuration (user + project level), approval policies, personality settings, enhanced slash commands, token tracking, shell prefix, rate limit handling, and context window management. After this phase, closed-code is a production-ready, highly configurable CLI.

**Depends on**: Phase 4 (Execute Mode — Diffs + Approvals)

---

## Phase Dependency Graph (within Phase 5)

```
5.1 Layered TOML Config
  │
  ├──► 5.2 Approval Policy Integration
  │
  ├──► 5.3 Personality System
  │
  └──► 5.4 Token Usage Tracking
         │
         ▼
       5.5 Enhanced REPL (depends on 5.1–5.4)
         │
         ▼
       5.6 Rate Limit Handling + Context Window
```

---

## Files Overview

```
src/
  config.rs              # REWRITE: layered TOML config + CLI + env merging
  error.rs               # EXTEND: ConfigError, InvalidApprovalPolicy, InvalidPersonality
  cli.rs                 # EXTEND: --approval-policy, --personality, --context-window-turns, --max-output-tokens
  agent/
    orchestrator.rs      # MODIFY: personality, session_usage, context_window_turns, model switching
  gemini/
    client.rs            # ENHANCE: rate limit retry with Retry-After + countdown
    stream.rs            # MODIFY: StreamResult carries UsageMetadata
  ui/
    mod.rs               # EXTEND: pub mod usage
    approval.rs          # EXTEND: PolicyAwareApprovalHandler
    usage.rs             # NEW: SessionUsage token tracker
  repl.rs                # MAJOR EXTENSION: all new slash commands, ! prefix, usage tracking
Cargo.toml               # ADD: toml = "0.8"
```

**Estimated lines**: ~1,500

---

## Sub-Phase 5.1: Layered TOML Configuration System

### Dependency to Add

```toml
toml = "0.8"
```

### File: `src/config.rs` (rewrite)

Replace the current minimal `Config` struct (6 fields, CLI-only resolution) with a full layered system.

**New enums:**

```rust
use serde::Deserialize;

/// Approval policy for file writes and shell commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Prompt for file writes AND shell commands (default).
    Suggest,
    /// Auto-approve file operations, prompt for shell commands.
    AutoEdit,
    /// Fully autonomous, no prompts.
    FullAuto,
}

/// Personality style that modifies the system prompt prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Personality {
    /// Warm, encouraging, casual language.
    Friendly,
    /// Direct, concise, code-focused.
    Pragmatic,
    /// Minimal personality, just answers.
    None,
}
```

Both implement `Default`, `Display`, and `FromStr`. `ApprovalPolicy` defaults to `Suggest`. `Personality` defaults to `Pragmatic`.

`FromStr` is case-insensitive and returns `ClosedCodeError::InvalidApprovalPolicy` / `ClosedCodeError::InvalidPersonality` on failure. Accept aliases: `"auto_edit"` and `"autoedit"`, `"full_auto"` and `"fullauto"`.

**TOML file struct (intermediate, all fields `Option` for layered merging):**

```rust
#[derive(Debug, Default, Deserialize)]
pub struct TomlConfig {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub default_mode: Option<String>,
    pub approval_policy: Option<String>,
    pub personality: Option<String>,
    pub context_window_turns: Option<usize>,
    pub max_output_tokens: Option<u32>,
    pub verbose: Option<bool>,
    #[serde(default)]
    pub shell: Option<ShellConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ShellConfig {
    pub additional_allowlist: Option<Vec<String>>,
}
```

**Final Config struct:**

```rust
#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub model: String,
    pub mode: Mode,
    pub working_directory: PathBuf,
    pub approval_policy: ApprovalPolicy,
    pub personality: Personality,
    pub shell_additional_allowlist: Vec<String>,
    pub context_window_turns: usize,    // default 50
    pub verbose: bool,
    pub max_output_tokens: u32,         // default 8192
}
```

**Resolution order** (later layers override earlier):

```
Hardcoded defaults
  ↓
~/.closed-code/config.toml       (user-level)
  ↓
<working_dir>/.closed-code/config.toml  (project-level)
  ↓
Environment variables (GEMINI_API_KEY)
  ↓
CLI flags (--api-key, --mode, --approval-policy, etc.)
```

**Key methods:**

```rust
impl Config {
    /// Build final Config from CLI args, layered TOML files, and env vars.
    pub fn from_cli(cli: &Cli) -> Result<Self> { ... }

    /// Load and parse a TOML file, returning None if it doesn't exist.
    fn load_toml_file(path: &Path) -> Result<Option<TomlConfig>> { ... }

    /// Merge two TomlConfig layers (overlay wins for present fields).
    fn merge(base: TomlConfig, overlay: TomlConfig) -> TomlConfig { ... }

    /// Load user config from ~/.closed-code/config.toml.
    fn load_user_config() -> Result<Option<TomlConfig>> { ... }

    /// Load project config from <working_dir>/.closed-code/config.toml.
    fn load_project_config(working_dir: &Path) -> Result<Option<TomlConfig>> { ... }
}
```

The `merge` function uses `Option::or()` for each field. For `ShellConfig`, the overlay replaces the base entirely if present.

**Example `~/.closed-code/config.toml`:**

```toml
model = "gemini-3.1-pro-preview"
approval_policy = "suggest"
personality = "pragmatic"
default_mode = "explore"
context_window_turns = 50

[shell]
additional_allowlist = ["docker", "cargo", "npm"]
```

### File: `src/error.rs` (extend)

Add three new error variants:

```rust
#[error("Configuration error: {0}")]
ConfigError(String),

#[error("Invalid approval policy '{0}'. Expected: suggest, auto_edit, full_auto")]
InvalidApprovalPolicy(String),

#[error("Invalid personality '{0}'. Expected: friendly, pragmatic, none")]
InvalidPersonality(String),
```

All three are non-retryable (`is_retryable() → false`).

### File: `src/cli.rs` (extend)

Add new optional CLI flags to the `Cli` struct:

```rust
/// Approval policy: suggest, auto_edit, full_auto
#[arg(long)]
pub approval_policy: Option<String>,

/// Personality: friendly, pragmatic, none
#[arg(long)]
pub personality: Option<String>,

/// Max context window turns before pruning
#[arg(long)]
pub context_window_turns: Option<usize>,

/// Max output tokens per response
#[arg(long)]
pub max_output_tokens: Option<u32>,
```

### Tests

- `config_from_cli_defaults` — verify all default values
- `config_from_cli_with_all_flags` — verify CLI overrides
- `config_toml_parsing` — parse a sample TOML string, verify all fields
- `config_merge_overlay_wins` — overlay fields override base
- `config_merge_none_preserves_base` — None fields don't clobber base
- `approval_policy_from_str` — all valid values + aliases
- `approval_policy_from_str_invalid` — error on bad input
- `personality_from_str` — all valid values
- `personality_from_str_invalid` — error on bad input
- `config_missing_api_key` — still returns `MissingApiKey` error

---

## Sub-Phase 5.2: Approval Policy Integration

### File: `src/ui/approval.rs` (extend)

The current `TerminalApprovalHandler` always prompts. Create a `PolicyAwareApprovalHandler` that wraps it with policy-based behavior.

**Key challenge**: The handler is stored as `Arc<dyn ApprovalHandler>` in the Orchestrator. The policy must be changeable at runtime via `/permissions`. Solution: use `std::sync::RwLock<ApprovalPolicy>` for interior mutability.

```rust
use std::sync::RwLock;
use crate::config::ApprovalPolicy;

/// Approval handler that applies an approval policy.
///
/// - Suggest: always prompt (delegates to inner TerminalApprovalHandler).
/// - AutoEdit: auto-approve file changes, prompt for shell commands.
/// - FullAuto: auto-approve everything.
#[derive(Debug)]
pub struct PolicyAwareApprovalHandler {
    policy: RwLock<ApprovalPolicy>,
    inner: TerminalApprovalHandler,
}

impl PolicyAwareApprovalHandler {
    pub fn new(policy: ApprovalPolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
            inner: TerminalApprovalHandler::new(),
        }
    }

    pub fn policy(&self) -> ApprovalPolicy {
        *self.policy.read().unwrap()
    }

    pub fn set_policy(&self, policy: ApprovalPolicy) {
        *self.policy.write().unwrap() = policy;
    }
}

#[async_trait]
impl ApprovalHandler for PolicyAwareApprovalHandler {
    async fn request_approval(&self, change: &FileChange) -> Result<ApprovalDecision> {
        match self.policy() {
            ApprovalPolicy::FullAuto | ApprovalPolicy::AutoEdit => {
                Ok(ApprovalDecision::Approved)
            }
            ApprovalPolicy::Suggest => self.inner.request_approval(change).await,
        }
    }
}
```

### Integration in `repl.rs`

Replace `TerminalApprovalHandler` with `PolicyAwareApprovalHandler`:

```rust
let approval_handler = Arc::new(PolicyAwareApprovalHandler::new(config.approval_policy));
```

The `Arc<PolicyAwareApprovalHandler>` is stored alongside the orchestrator so `/permissions` can call `set_policy()` on it at runtime.

### Tests

- `policy_suggest_delegates` — Suggest policy calls inner handler
- `policy_auto_edit_approves` — AutoEdit returns Approved without prompting
- `policy_full_auto_approves` — FullAuto returns Approved without prompting
- `policy_runtime_switch` — Change policy after construction, verify behavior changes
- `policy_thread_safety` — Read policy from multiple threads concurrently

---

## Sub-Phase 5.3: Personality System

### File: `src/agent/orchestrator.rs` (modify)

**New field**: `personality: Personality`

**Updated constructor**:

```rust
pub fn new(
    client: Arc<GeminiClient>,
    mode: Mode,
    working_directory: PathBuf,
    max_output_tokens: u32,
    approval_handler: Arc<dyn ApprovalHandler>,
    personality: Personality,         // NEW
    context_window_turns: usize,      // NEW (from 5.4)
) -> Self { ... }
```

**Updated `build_system_prompt`**:

```rust
fn build_system_prompt(
    mode: &Mode,
    working_directory: &Path,
    personality: Personality,
) -> String {
    let personality_prefix = match personality {
        Personality::Friendly => {
            "You are warm, encouraging, and approachable. Use casual but \
             professional language. Celebrate progress and be supportive \
             when users encounter issues.\n\n"
        }
        Personality::Pragmatic => {
            "You are direct, concise, and code-focused. Get straight to \
             the point. Prioritize accuracy and efficiency in your responses.\n\n"
        }
        Personality::None => "",
    };

    let base = format!(
        "{}You are closed-code, an AI coding assistant operating in {} mode.\n\
         Working directory: {}",
        personality_prefix,
        mode,
        working_directory.display()
    );

    // ... mode_section unchanged ...
    format!("{}{}", base, mode_section)
}
```

**New methods**:

```rust
pub fn personality(&self) -> Personality {
    self.personality
}

pub fn set_personality(&mut self, personality: Personality) {
    self.personality = personality;
    self.system_prompt = Self::build_system_prompt(
        &self.mode,
        &self.working_directory,
        self.personality,
    );
}
```

**Updated `set_mode`**: Must also pass `self.personality` to `build_system_prompt`.

### Tests

- `orchestrator_friendly_prompt` — system prompt contains warm language
- `orchestrator_pragmatic_prompt` — system prompt contains direct language
- `orchestrator_none_prompt` — system prompt has no personality prefix
- `set_personality_rebuilds_prompt` — changing personality updates the prompt
- `set_mode_preserves_personality` — mode switch keeps current personality

---

## Sub-Phase 5.4: Token Usage Tracking

### New File: `src/ui/usage.rs`

```rust
use crate::gemini::types::UsageMetadata;

/// Cumulative token usage tracker for a session.
#[derive(Debug, Default, Clone)]
pub struct SessionUsage {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub api_calls: u64,
}

impl SessionUsage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulate usage from a single API response.
    pub fn accumulate(&mut self, usage: &UsageMetadata) {
        self.total_prompt_tokens += usage.prompt_token_count.unwrap_or(0) as u64;
        self.total_completion_tokens += usage.candidates_token_count.unwrap_or(0) as u64;
        self.total_tokens += usage.total_token_count.unwrap_or(0) as u64;
        self.api_calls += 1;
    }
}

impl std::fmt::Display for SessionUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} prompt + {} completion = {} total ({} API calls)",
            format_number(self.total_prompt_tokens),
            format_number(self.total_completion_tokens),
            format_number(self.total_tokens),
            self.api_calls,
        )
    }
}

/// Format a number with comma separators: 1234567 → "1,234,567"
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
```

### File: `src/ui/mod.rs` (extend)

```rust
pub mod usage;
```

### File: `src/gemini/stream.rs` (modify)

The current `StreamResult` enum does not carry usage metadata:

```rust
pub enum StreamResult {
    Text(String),
    FunctionCall {
        text_so_far: String,
        response: GenerateContentResponse,
    },
}
```

Extend both variants to include usage:

```rust
pub enum StreamResult {
    Text {
        text: String,
        usage: Option<UsageMetadata>,
    },
    FunctionCall {
        text_so_far: String,
        response: GenerateContentResponse,
        usage: Option<UsageMetadata>,
    },
}
```

Update `consume_stream` to capture the `UsageMetadata` from `StreamEvent::Done` and include it in the returned `StreamResult`.

### File: `src/agent/orchestrator.rs` (extend)

**New field**: `session_usage: SessionUsage`

After each `consume_stream` call in both `handle_user_input_streaming` and `run_tool_loop`, accumulate the returned usage:

```rust
let stream_result = consume_stream(es, |event| { ... }).await?;

// Extract and accumulate usage
match &stream_result {
    StreamResult::Text { usage, .. } |
    StreamResult::FunctionCall { usage, .. } => {
        if let Some(u) = usage {
            self.session_usage.accumulate(u);
        }
    }
}
```

**New getter**:

```rust
pub fn session_usage(&self) -> &SessionUsage {
    &self.session_usage
}
```

### Tests

- `session_usage_default` — starts at zero
- `session_usage_accumulate` — adds tokens correctly
- `session_usage_multiple_accumulate` — multiple calls sum correctly
- `session_usage_display` — formats with commas
- `format_number_basic` — 0, 42, 1000, 1234567
- `stream_result_carries_usage` — consume_stream returns usage in both variants

---

## Sub-Phase 5.5: Enhanced REPL

### File: `src/repl.rs` (major extension)

**Refactor `handle_slash_command`**: Currently uses exact `match` on full input strings. Refactor to split command and argument:

```rust
fn handle_slash_command(
    input: &str,
    orchestrator: &mut Orchestrator,
    approval_handler: &PolicyAwareApprovalHandler,
) -> SlashResult {
    let (cmd, arg) = match input.find(' ') {
        Some(pos) => (&input[..pos], input[pos + 1..].trim()),
        None => (input, ""),
    };

    match cmd {
        "/quit" | "/exit" | "/q" => SlashResult::Quit,
        "/clear" => { ... }
        "/accept" | "/a" => { ... }
        "/help" => { /* updated help text with all commands */ }

        "/model" => {
            if arg.is_empty() {
                println!("Current model: {}", orchestrator.model());
            } else {
                orchestrator.set_model(arg.to_string());
                println!("Model changed to: {}", arg);
            }
            SlashResult::Continue
        }

        "/permissions" => {
            if arg.is_empty() {
                println!("Current approval policy: {}", approval_handler.policy());
            } else {
                match arg.parse::<ApprovalPolicy>() {
                    Ok(policy) => {
                        approval_handler.set_policy(policy);
                        println!("Approval policy changed to: {}", policy);
                    }
                    Err(e) => println!("{}", e),
                }
            }
            SlashResult::Continue
        }

        "/personality" => {
            if arg.is_empty() {
                println!("Current personality: {}", orchestrator.personality());
            } else {
                match arg.parse::<Personality>() {
                    Ok(p) => {
                        orchestrator.set_personality(p);
                        println!("Personality changed to: {}", p);
                    }
                    Err(e) => println!("{}", e),
                }
            }
            SlashResult::Continue
        }

        "/status" => {
            println!(
                "Mode: {} | Model: {} | Policy: {} | Personality: {}",
                orchestrator.mode(),
                orchestrator.model(),
                approval_handler.policy(),
                orchestrator.personality(),
            );
            println!("Tokens: {}", orchestrator.session_usage());
            println!(
                "Turns: {} / {} | Tools: {}",
                orchestrator.turn_count(),
                orchestrator.context_window_turns(),
                orchestrator.tool_count(),
            );
            SlashResult::Continue
        }

        "/explore" => {
            orchestrator.set_mode(Mode::Explore);
            println!("Switched to explore mode. Tools: {}", orchestrator.tool_count());
            SlashResult::Continue
        }

        "/plan" => {
            orchestrator.set_mode(Mode::Plan);
            println!("Switched to plan mode. Tools: {}", orchestrator.tool_count());
            SlashResult::Continue
        }

        "/execute" => {
            orchestrator.set_mode(Mode::Execute);
            println!("Switched to execute mode. Tools: {}", orchestrator.tool_count());
            SlashResult::Continue
        }

        "/mode" => { /* existing /mode [name] logic */ }

        _ => {
            println!("Unknown command: {}. Type /help for available commands.", input);
            SlashResult::Continue
        }
    }
}
```

### New Slash Commands Summary

| Command | Description |
|---------|-------------|
| `/model [name]` | Show or switch model |
| `/permissions [policy]` | Show or change approval policy (suggest, auto_edit, full_auto) |
| `/personality [style]` | Show or change personality (friendly, pragmatic, none) |
| `/status` | Token usage, model, mode, policy, personality, turns, tools |
| `/explore` | Switch to Explore mode (shorthand for `/mode explore`) |
| `/plan` | Switch to Plan mode (shorthand for `/mode plan`) |
| `/execute` | Switch to Execute mode (shorthand for `/mode execute`) |
| `/mode [name]` | Show or switch mode (existing) |
| `/clear` | Clear conversation history (existing) |
| `/accept` | Accept plan and switch to Execute (existing) |
| `/help` | Show all commands (updated) |
| `/quit` | Exit (existing) |

### Shell Prefix (`!`)

In the main REPL loop, before checking for `/` prefix, check for `!`:

```rust
if line.starts_with('!') {
    let shell_cmd = &line[1..];
    if !shell_cmd.is_empty() {
        match execute_local_shell(shell_cmd, &config.working_directory).await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stdout.is_empty() { print!("{}", stdout); }
                if !stderr.is_empty() { eprint!("{}", stderr); }
            }
            Err(e) => eprintln!("{}: {}", styled_text("Error", Theme::ERROR), e),
        }
    }
    continue;
}
```

Helper function:

```rust
async fn execute_local_shell(
    cmd: &str,
    working_directory: &Path,
) -> anyhow::Result<std::process::Output> {
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_directory)
        .output()
        .await
        .map_err(Into::into)
}
```

**Important**: The `!` shell prefix does NOT go through the tool allowlist — it runs arbitrary local commands. This is intentional because the user is typing them directly (not the LLM). The output is NOT sent to the LLM.

### Inline Stream Handler for Usage

Replace the free function `default_stream_handler` with an inline closure that also accumulates token usage. Since `SessionUsage` now lives in the Orchestrator, the stream handler callback in the REPL just handles display:

```rust
match orchestrator
    .handle_user_input_streaming(line, |event| {
        match event {
            StreamEvent::TextDelta(text) => {
                print!("{}", text);
                std::io::stdout().flush().ok();
            }
            StreamEvent::Done { usage, .. } => {
                println!();
                if let Some(u) = &usage {
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
    })
    .await
```

Usage accumulation happens inside the Orchestrator (Sub-Phase 5.4), so the REPL callback only needs to handle display.

### Model Switching

The Orchestrator needs a `set_model` method. Since `GeminiClient` stores the model as an owned `String` behind `Arc`, switching models requires creating a new `Arc<GeminiClient>`:

```rust
// In Orchestrator:
pub fn set_model(&mut self, model: String) {
    self.model_name = model.clone();
    self.client = Arc::new(GeminiClient::new(
        self.client.api_key().to_string(), // needs a getter
        model,
    ));
    // Rebuild registry with new client (sub-agent spawn tools hold client ref)
    self.registry = create_orchestrator_registry(
        self.working_directory.clone(),
        &self.mode,
        self.client.clone(),
        Some(self.approval_handler.clone()),
    );
}

pub fn model(&self) -> &str {
    &self.model_name
}
```

Add `pub fn api_key(&self) -> &str` to `GeminiClient` (redacted in Debug but accessible for reconstruction).

### Tests

- `slash_model_show` — `/model` with no arg prints current model
- `slash_model_switch` — `/model gemini-2.0-flash` changes the model
- `slash_permissions_show` — `/permissions` prints current policy
- `slash_permissions_switch` — `/permissions full_auto` changes policy
- `slash_personality_show` — `/personality` prints current personality
- `slash_personality_switch` — `/personality friendly` changes personality
- `slash_status_output` — `/status` prints all session info
- `slash_explore_shorthand` — `/explore` switches to explore mode
- `slash_plan_shorthand` — `/plan` switches to plan mode
- `slash_execute_shorthand` — `/execute` switches to execute mode
- `slash_command_arg_splitting` — command/arg split works for all commands
- `shell_prefix_detection` — `!ls` is recognized as shell command
- `slash_help_lists_all_commands` — help output includes all new commands

---

## Sub-Phase 5.6: Rate Limit Handling + Context Window Management

### File: `src/gemini/client.rs` (enhance)

**Rate limit retry with `Retry-After` header parsing:**

The current retry logic uses `backon` for exponential backoff on 429 and 5xx. Enhance specifically for 429 responses:

```rust
/// Parse Retry-After header from a 429 response.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Add jitter to a duration (±25%).
fn with_jitter(duration: Duration) -> Duration {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let jitter_factor = 0.75 + (nanos as f64 / u32::MAX as f64) * 0.5; // 0.75–1.25
    Duration::from_millis((duration.as_millis() as f64 * jitter_factor) as u64)
}
```

For the streaming endpoint, rate limits typically happen at connection time. When a 429 error surfaces from `stream_generate_content`, catch it at the orchestrator level and retry with countdown display.

**Orchestrator-level rate limit handling** (in `handle_user_input_streaming` and `run_tool_loop`):

```rust
// When the stream or API call returns a RateLimited error:
Err(ClosedCodeError::RateLimited { retry_after_ms }) => {
    let delay = Duration::from_millis(retry_after_ms);
    display_rate_limit_countdown(delay).await;
    // Retry the request (up to 5 times for rate limits)
    continue;
}
```

**Countdown display function** (in `repl.rs` or `ui/` module):

```rust
async fn display_rate_limit_countdown(delay: Duration) {
    let secs = delay.as_secs();
    for remaining in (1..=secs).rev() {
        eprint!("\rRate limited. Retrying in {}s... ", remaining);
        std::io::stderr().flush().ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    eprintln!("\rRetrying now...                    ");
}
```

### Context Window Management

**File: `src/agent/orchestrator.rs` (modify)**

Replace the hardcoded `MAX_CONTEXT_TURNS = 50` with the configurable `context_window_turns` field:

```rust
pub fn prune_history(&mut self) {
    if self.history.len() <= self.context_window_turns {
        return;
    }

    let keep = self.context_window_turns / 2;
    let pruned_count = self.history.len() - keep;
    self.history = self.history.split_off(self.history.len() - keep);

    // Ensure first message is from user
    let first_is_user = self
        .history
        .first()
        .and_then(|c| c.role.as_deref())
        .map(|r| r == "user")
        .unwrap_or(false);

    if !first_is_user {
        self.history.insert(
            0,
            Content::user("[Earlier conversation context was pruned]"),
        );
    }

    // Notification printed to stderr (not part of LLM conversation)
    eprintln!(
        "Context pruned: removed {} oldest turns ({} remaining)",
        pruned_count,
        self.history.len()
    );
}
```

**New getter**:

```rust
pub fn context_window_turns(&self) -> usize {
    self.context_window_turns
}
```

**Context warning at 80%**: When `self.history.len()` exceeds `80%` of `context_window_turns`, print a one-time warning:

```rust
let threshold = (self.context_window_turns as f64 * 0.8) as usize;
if self.history.len() == threshold {
    eprintln!(
        "Warning: Approaching context limit ({}/{} turns). Consider /clear or conversation will be pruned.",
        self.history.len(),
        self.context_window_turns,
    );
}
```

### Tests

- `retry_after_parsing` — parse "5" → Duration::from_secs(5)
- `retry_after_missing` — no header → None
- `jitter_within_range` — result is within 75%–125% of input
- `prune_configurable_turns` — pruning respects custom context_window_turns
- `context_window_getter` — returns configured value

---

## Orchestrator Changes Summary

The `Orchestrator` struct after Phase 5:

```rust
pub struct Orchestrator {
    client: Arc<GeminiClient>,
    mode: Mode,
    working_directory: PathBuf,
    history: Vec<Content>,
    registry: ToolRegistry,
    system_prompt: String,
    max_output_tokens: u32,
    approval_handler: Arc<dyn ApprovalHandler>,
    current_plan: Option<String>,
    cancelled: Arc<AtomicBool>,
    // NEW in Phase 5:
    personality: Personality,
    context_window_turns: usize,
    session_usage: SessionUsage,
    model_name: String,
}
```

**New constructor signature** (breaking change from Phase 4):

```rust
pub fn new(
    client: Arc<GeminiClient>,
    mode: Mode,
    working_directory: PathBuf,
    max_output_tokens: u32,
    approval_handler: Arc<dyn ApprovalHandler>,
    personality: Personality,
    context_window_turns: usize,
) -> Self
```

**New public methods**:

| Method | Description |
|--------|-------------|
| `personality(&self) -> Personality` | Get current personality |
| `set_personality(&mut self, p: Personality)` | Set personality, rebuilds system prompt |
| `model(&self) -> &str` | Get current model name |
| `set_model(&mut self, model: String)` | Switch model, rebuilds client + registry |
| `session_usage(&self) -> &SessionUsage` | Get cumulative token usage |
| `context_window_turns(&self) -> usize` | Get configured context window size |

---

## Milestone

```bash
# Config file respected
mkdir -p ~/.closed-code
echo 'personality = "friendly"' > ~/.closed-code/config.toml
cargo run
# > Hi!
# Hey there! Great to see you! What can I help with today?

# Token tracking
# > /status
# Mode: explore | Model: gemini-3.1-pro-preview | Policy: suggest | Personality: friendly
# Tokens: 1,234 prompt + 567 completion = 1,801 total (3 API calls)
# Turns: 4 / 50 | Tools: 6

# Approval policy change
# > /permissions auto_edit
# Approval policy changed to: auto_edit

# Personality change
# > /personality pragmatic
# Personality changed to: pragmatic

# Model switch
# > /model gemini-2.0-flash
# Model changed to: gemini-2.0-flash

# Mode shortcuts
# > /plan
# Switched to plan mode. Tools: 8

# Shell prefix
# > !git status
# On branch main
# Changes not staged for commit: ...

# Rate limit handling
# > (after many requests)
# Rate limited. Retrying in 3s...
# Rate limited. Retrying in 7s...
# (response arrives)

# Context pruning
# > (after 40+ turns)
# Warning: Approaching context limit (40/50 turns). Consider /clear or conversation will be pruned.
# > (after 50+ turns)
# Context pruned: removed 25 oldest turns (25 remaining)
```

---

## Implementation Order

1. **5.1 — Config system** (`config.rs` rewrite, `error.rs`, `cli.rs`) — foundation for everything else
2. **5.2 — Approval policy** (`ui/approval.rs`) — depends on `ApprovalPolicy` enum from 5.1
3. **5.3 — Personality** (`orchestrator.rs`) — depends on `Personality` enum from 5.1
4. **5.4 — Token tracking** (`ui/usage.rs`, `gemini/stream.rs`, `orchestrator.rs`) — independent of 5.2/5.3
5. **5.5 — Enhanced REPL** (`repl.rs`) — depends on 5.1–5.4 (uses all new types)
6. **5.6 — Rate limit + context** (`gemini/client.rs`, `orchestrator.rs`) — can be done in parallel with 5.5

---

## Complexity: **Medium-High**

Extending existing systems rather than building new ones. No new major abstractions — layered config, enhanced REPL dispatch, and usage tracking are mostly straightforward. The main complexity is in the number of integration points across files. ~12 files modified, ~1,500 lines.

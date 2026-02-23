# Phase 7: Sandboxing + Security Hardening

**Goal**: Platform-specific sandboxing for shell commands. macOS uses Seatbelt (`sandbox-exec`), Linux uses Landlock filesystem restrictions. Multiple sandbox modes with extended, configurable protected path enforcement.

**Depends on**: Phase 5 (Configuration + Enhanced REPL)

---

## Phase Dependency Graph (within Phase 7)

```
7.1 Sandbox Module Foundation (trait, enum, factory)
  │
  ├──► 7.2 macOS Seatbelt Backend
  │
  ├──► 7.3 Linux Landlock Backend
  │
  ├──► 7.4 Fallback Backend
  │
  └──► 7.5 Shell Tool Integration (route through sandbox)
              │
              ├──► 7.6 Extended Protected Paths (configurable)
              │
              ├──► 7.7 Config + CLI Changes (--sandbox, [security])
              │         │
              │         ▼
              └──► 7.8 /sandbox Slash Command
```

---

## Files Overview

```
src/
  sandbox/
    mod.rs             # NEW: Sandbox trait, SandboxMode enum, SandboxBackend enum, create_sandbox() factory
    macos.rs           # NEW: SeatbeltSandbox — dynamic profile generation, sandbox-exec invocation
    linux.rs           # NEW: LandlockSandbox — Landlock ruleset creation, pre_exec hook
    fallback.rs        # NEW: FallbackSandbox — unsandboxed execution with tracing warning
  tool/
    mod.rs             # MODIFIED: Shared is_protected_path() moved here, accepts configurable paths
    shell.rs           # MODIFIED: ShellCommandTool gains Arc<dyn Sandbox>, routes execution through it
    registry.rs        # MODIFIED: Factory functions accept Arc<dyn Sandbox>, thread through to ShellCommandTool
    file_write.rs      # MODIFIED: Uses shared is_protected_path() from tool/mod.rs
    file_edit.rs       # MODIFIED: Uses shared is_protected_path() from tool/mod.rs
  agent/
    orchestrator.rs    # MODIFIED: Stores Arc<dyn Sandbox>, passes to registry, sandbox info in system prompt
  config.rs            # MODIFIED: New SecurityConfig, sandbox_mode field, protected_paths
  cli.rs               # MODIFIED: New --sandbox flag
  error.rs             # MODIFIED: New SandboxDenied, SandboxUnavailable variants
  repl.rs              # MODIFIED: /sandbox slash command, startup banner shows sandbox mode
  lib.rs               # MODIFIED: Added pub mod sandbox;
```

### New Cargo Dependencies

```toml
# Platform-specific (Linux only)
[target.'cfg(target_os = "linux")'.dependencies]
landlock = "0.4"
```

No additional dependencies for macOS — Seatbelt uses the system `sandbox-exec` binary. No additional dependencies for the fallback — it reuses existing `tokio::process::Command`.

---

## Sub-Phase 7.1: Sandbox Module Foundation

### New File: `src/sandbox/mod.rs`

Module re-exports, core types, and the platform-dispatch factory:

```rust
pub mod fallback;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "linux")]
pub mod linux;

pub use fallback::FallbackSandbox;
#[cfg(target_os = "macos")]
pub use macos::SeatbeltSandbox;
#[cfg(target_os = "linux")]
pub use linux::LandlockSandbox;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Output;

use async_trait::async_trait;

use crate::error::Result;
```

**SandboxMode enum:**

```rust
/// Sandbox restriction level for shell command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Reads and writes restricted to workspace + system essentials. Network blocked.
    /// Strongest isolation — the agent cannot read files outside the project.
    WorkspaceOnly,
    /// Reads everywhere, writes only within workspace + /tmp. Network allowed.
    /// The invisible default — prevents accidental writes outside the project
    /// while allowing normal development workflows (cargo build, git push, etc.).
    WorkspaceWrite,
    /// No sandbox restrictions (requires explicit opt-in).
    FullAccess,
}

impl Default for SandboxMode {
    fn default() -> Self {
        Self::WorkspaceWrite
    }
}
```

`Display` and `FromStr` implementations:

| String | Variant |
|--------|---------|
| `"workspace-only"` or `"workspace_only"` | `WorkspaceOnly` |
| `"workspace-write"` or `"workspace_write"` | `WorkspaceWrite` |
| `"full-access"` or `"full_access"` | `FullAccess` |

**SandboxBackend enum:**

```rust
/// Identifies which platform sandbox backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    Seatbelt,    // macOS sandbox-exec
    Landlock,    // Linux Landlock LSM
    Fallback,    // No OS-level sandbox
}

impl fmt::Display for SandboxBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Seatbelt => write!(f, "macOS Seatbelt"),
            Self::Landlock => write!(f, "Linux Landlock"),
            Self::Fallback => write!(f, "fallback (unsandboxed)"),
        }
    }
}
```

**Sandbox trait:**

```rust
#[async_trait]
pub trait Sandbox: Send + Sync + fmt::Debug {
    /// Execute a command with sandbox restrictions applied.
    ///
    /// `command` is the executable path/name, `args` are its arguments,
    /// `cwd` is the working directory for the child process.
    async fn execute_command(
        &self,
        command: &str,
        args: &[String],
        cwd: &Path,
    ) -> Result<Output>;

    /// The sandbox restriction level.
    fn mode(&self) -> SandboxMode;

    /// Which platform backend this sandbox uses.
    fn backend(&self) -> SandboxBackend;

    /// The workspace directory that WorkspaceWrite mode permits writes to.
    fn workspace(&self) -> &Path;
}
```

**Factory function:**

```rust
/// Create the appropriate sandbox for the current platform and mode.
///
/// - FullAccess always returns FallbackSandbox (no restrictions needed).
/// - macOS: returns SeatbeltSandbox if sandbox-exec is available.
/// - Linux: returns LandlockSandbox if Landlock is supported by the kernel.
/// - Otherwise: returns FallbackSandbox with a tracing warning.
pub fn create_sandbox(
    mode: SandboxMode,
    workspace: PathBuf,
) -> Arc<dyn Sandbox> {
    if mode == SandboxMode::FullAccess {
        return Arc::new(FallbackSandbox::new(mode, workspace));
    }

    #[cfg(target_os = "macos")]
    {
        if SeatbeltSandbox::is_available() {
            return Arc::new(SeatbeltSandbox::new(mode, workspace));
        }
        tracing::warn!("sandbox-exec not found; falling back to unsandboxed execution");
    }

    #[cfg(target_os = "linux")]
    {
        if LandlockSandbox::is_supported() {
            return Arc::new(LandlockSandbox::new(mode, workspace));
        }
        tracing::warn!("Landlock not supported by kernel; falling back to unsandboxed execution");
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        tracing::warn!("Sandboxing not available on this platform; running without sandbox");
    }

    Arc::new(FallbackSandbox::new(mode, workspace))
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| `SandboxMode` is immutable at runtime | Security: prevents escalation mid-session. The LLM cannot persuade the user to weaken protections. |
| `Arc<dyn Sandbox>` sharing | Mirrors the existing `Arc<GeminiClient>` and `Arc<dyn ApprovalHandler>` patterns. |
| FullAccess always uses Fallback | No point wrapping with sandbox-exec just to allow everything. |
| Factory handles detection | Centralizes platform checks; callers just get an `Arc<dyn Sandbox>`. |
| `WorkspaceWrite` allows network | The invisible default must not break `cargo build`, `git push`, or any normal development workflow. Write restrictions are the primary value; network blocking is only for `WorkspaceOnly`. |
| `WorkspaceOnly` is the high-security mode | Restricts both reads and writes to the workspace. This is where sandboxing earns its keep — the LLM cannot access `~/.ssh`, `~/.aws`, other projects, etc. |

### `src/lib.rs`

Add `pub mod sandbox;` to module declarations.

---

## Sub-Phase 7.2: macOS Seatbelt Backend

### New File: `src/sandbox/macos.rs`

The Seatbelt sandbox uses macOS's built-in `sandbox-exec` to run commands under a dynamically-generated Seatbelt profile.

```rust
use std::path::{Path, PathBuf};
use std::process::Output;

use async_trait::async_trait;
use tokio::process::Command;

use super::{Sandbox, SandboxBackend, SandboxMode};
use crate::error::{ClosedCodeError, Result};

#[derive(Debug)]
pub struct SeatbeltSandbox {
    mode: SandboxMode,
    workspace: PathBuf,
    profile: String,
}
```

**Constructor and availability check:**

```rust
impl SeatbeltSandbox {
    pub fn new(mode: SandboxMode, workspace: PathBuf) -> Self {
        let profile = Self::generate_profile(mode, &workspace);
        Self {
            mode,
            workspace,
            profile,
        }
    }

    /// Check if sandbox-exec is available on this system.
    pub fn is_available() -> bool {
        std::process::Command::new("sandbox-exec")
            .arg("-n")
            .arg("no-network")
            .arg("true")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }
}
```

**Profile generation:**

```rust
impl SeatbeltSandbox {
    /// Generate a Seatbelt profile string for the given mode.
    fn generate_profile(mode: SandboxMode, workspace: &Path) -> String {
        match mode {
            SandboxMode::WorkspaceOnly => Self::workspace_only_profile(workspace),
            SandboxMode::WorkspaceWrite => Self::workspace_write_profile(workspace),
            SandboxMode::FullAccess => unreachable!("FullAccess uses FallbackSandbox"),
        }
    }

    fn workspace_only_profile(workspace: &Path) -> String {
        let workspace_path = workspace.to_string_lossy();
        format!(
            r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow sysctl-read)
(allow mach-lookup)
(allow signal (target self))
;; Read access: workspace + system essentials only
(allow file-read* (subpath "{workspace_path}"))
(allow file-read* (subpath "/usr"))
(allow file-read* (subpath "/bin"))
(allow file-read* (subpath "/sbin"))
(allow file-read* (subpath "/opt/homebrew"))
(allow file-read* (subpath "/Library"))
(allow file-read* (subpath "/System"))
(allow file-read* (subpath "/etc"))
(allow file-read* (subpath "/private/etc"))
(allow file-read* (subpath "/private/tmp"))
(allow file-read* (subpath "/tmp"))
(allow file-read* (subpath "/dev"))
(allow file-read* (subpath "/var"))
;; Write access: workspace + /tmp only
(allow file-write* (subpath "{workspace_path}"))
(allow file-write* (subpath "/private/tmp"))
(allow file-write* (subpath "/tmp"))
(deny network*)"#
        )
    }

    fn workspace_write_profile(workspace: &Path) -> String {
        let workspace_path = workspace.to_string_lossy();
        format!(
            r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow sysctl-read)
(allow mach-lookup)
(allow signal (target self))
(allow file-read*)
(allow file-write* (subpath "{workspace_path}"))
(allow file-write* (subpath "/private/tmp"))
(allow file-write* (subpath "/tmp"))
(allow network*)"#
        )
    }
}
```

**Seatbelt profile details:**

| Mode | file-read | file-write | network |
|------|-----------|------------|---------|
| `WorkspaceOnly` | Workspace + system paths only | Workspace + `/tmp` | `deny network*` |
| `WorkspaceWrite` | `allow file-read*` (everywhere) | Workspace + `/tmp` | `allow network*` |

Both profiles allow `process-exec`, `process-fork`, `sysctl-read`, and `mach-lookup` — required for running any command. `/tmp` write access is included because many commands use temporary files.

**System paths allowed for reads in `WorkspaceOnly`:**

| Path | Reason |
|------|--------|
| `/usr`, `/bin`, `/sbin` | Executables and shared libraries |
| `/opt/homebrew` | Homebrew-installed tools (common on macOS) |
| `/Library`, `/System` | macOS frameworks and system libraries |
| `/etc`, `/private/etc` | System configuration (e.g., `/etc/gitconfig`) |
| `/tmp`, `/private/tmp` | Temporary files |
| `/dev` | Device files (`/dev/null`, `/dev/urandom`) |
| `/var` | System state (logs, sockets needed by some tools) |

Notably **absent**: `$HOME` (except the workspace), other user directories, other projects. This is the key security benefit — the LLM cannot read `~/.ssh`, `~/.aws`, `~/other-project`, etc.

**Trait implementation:**

```rust
#[async_trait]
impl Sandbox for SeatbeltSandbox {
    async fn execute_command(
        &self,
        command: &str,
        args: &[String],
        cwd: &Path,
    ) -> Result<Output> {
        // Resolve the full path of the command for sandbox-exec
        let resolved_cmd = Self::resolve_command(command)?;

        let output = Command::new("sandbox-exec")
            .arg("-p")
            .arg(&self.profile)
            .arg(&resolved_cmd)
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| ClosedCodeError::ShellError(
                format!("Seatbelt execution failed for '{}': {}", command, e),
            ))?;

        // Check if the sandbox itself denied the operation
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("deny") || stderr.contains("Sandbox") {
                return Err(ClosedCodeError::SandboxDenied {
                    command: command.to_string(),
                    reason: format!("Seatbelt denied operation (mode: {})", self.mode),
                });
            }
        }

        Ok(output)
    }

    fn mode(&self) -> SandboxMode {
        self.mode
    }

    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Seatbelt
    }

    fn workspace(&self) -> &Path {
        &self.workspace
    }
}
```

**Command resolution helper:**

```rust
impl SeatbeltSandbox {
    /// Resolve a command name to its full path using `which`.
    /// sandbox-exec works more reliably with absolute paths.
    fn resolve_command(command: &str) -> Result<String> {
        // If it's already an absolute path, use it directly
        if command.starts_with('/') {
            return Ok(command.to_string());
        }

        // Use `which` to resolve
        let output = std::process::Command::new("which")
            .arg(command)
            .output()
            .map_err(|e| ClosedCodeError::ShellError(
                format!("Failed to resolve command '{}': {}", command, e),
            ))?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(path)
        } else {
            // Fall back to the bare command name
            Ok(command.to_string())
        }
    }
}
```

---

## Sub-Phase 7.3: Linux Landlock Backend

### New File: `src/sandbox/linux.rs`

Uses the `landlock` crate to restrict filesystem access on the child process via `pre_exec`.

```rust
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use async_trait::async_trait;
use landlock::{
    Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, ABI,
};
use tokio::process::Command;

use super::{Sandbox, SandboxBackend, SandboxMode};
use crate::error::{ClosedCodeError, Result};

/// Minimum Landlock ABI version we require.
const MIN_ABI: ABI = ABI::V1;

#[derive(Debug)]
pub struct LandlockSandbox {
    mode: SandboxMode,
    workspace: PathBuf,
}
```

**Constructor and support check:**

```rust
impl LandlockSandbox {
    pub fn new(mode: SandboxMode, workspace: PathBuf) -> Self {
        Self { mode, workspace }
    }

    /// Check if the running kernel supports Landlock.
    pub fn is_supported() -> bool {
        // Attempt to create a minimal ruleset to test support
        Ruleset::default()
            .handle_access(AccessFs::from_all(MIN_ABI))
            .is_ok()
    }
}
```

**Ruleset creation per mode:**

```rust
impl LandlockSandbox {
    /// Build a Landlock ruleset for the given mode.
    /// Returns a closure suitable for use with `pre_exec`.
    fn build_ruleset(&self) -> impl FnMut() -> std::io::Result<()> + Send {
        let mode = self.mode;
        let workspace = self.workspace.clone();

        move || {
            let read_access = AccessFs::from_read(MIN_ABI);
            let write_access = AccessFs::from_write(MIN_ABI);

            let mut ruleset = Ruleset::default()
                .handle_access(AccessFs::from_all(MIN_ABI))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
                .create()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            match mode {
                SandboxMode::WorkspaceOnly => {
                    // Reads: workspace + system essentials only
                    let read_paths = [
                        workspace.as_path(),
                        Path::new("/usr"),
                        Path::new("/bin"),
                        Path::new("/sbin"),
                        Path::new("/lib"),
                        Path::new("/lib64"),
                        Path::new("/etc"),
                        Path::new("/tmp"),
                        Path::new("/dev"),
                        Path::new("/proc"),
                        Path::new("/sys"),
                        Path::new("/var"),
                        Path::new("/run"),
                    ];
                    for path in &read_paths {
                        if path.exists() {
                            ruleset = ruleset
                                .add_rule(PathBeneath::new(
                                    PathFd::new(path)
                                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
                                    read_access,
                                ))
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        }
                    }

                    // Writes: workspace + /tmp only
                    ruleset = ruleset
                        .add_rule(PathBeneath::new(
                            PathFd::new(&workspace)
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
                            write_access,
                        ))
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                    ruleset = ruleset
                        .add_rule(PathBeneath::new(
                            PathFd::new("/tmp")
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
                            write_access,
                        ))
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                }
                SandboxMode::WorkspaceWrite => {
                    // Reads: everywhere
                    ruleset = ruleset
                        .add_rule(PathBeneath::new(
                            PathFd::new("/")
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
                            read_access,
                        ))
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                    // Writes: workspace + /tmp only
                    ruleset = ruleset
                        .add_rule(PathBeneath::new(
                            PathFd::new(&workspace)
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
                            write_access,
                        ))
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                    ruleset = ruleset
                        .add_rule(PathBeneath::new(
                            PathFd::new("/tmp")
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
                            write_access,
                        ))
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                }
                SandboxMode::FullAccess => unreachable!("FullAccess uses FallbackSandbox"),
            }

            let status = ruleset
                .restrict_self()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            if status.ruleset == RulesetStatus::NotEnforced {
                // Kernel too old or Landlock disabled — log but don't fail
                eprintln!("Warning: Landlock ruleset was not enforced by kernel");
            }

            Ok(())
        }
    }
}
```

**Trait implementation:**

```rust
#[async_trait]
impl Sandbox for LandlockSandbox {
    async fn execute_command(
        &self,
        command: &str,
        args: &[String],
        cwd: &Path,
    ) -> Result<Output> {
        let mut ruleset_fn = self.build_ruleset();

        // pre_exec runs after fork() but before exec() in the child process.
        // This applies Landlock restrictions only to the child, not our process.
        let output = unsafe {
            Command::new(command)
                .args(args)
                .current_dir(cwd)
                .pre_exec(move || ruleset_fn())
                .output()
                .await
        }
        .map_err(|e| {
            let stderr = e.to_string();
            if stderr.contains("Permission denied") || stderr.contains("EACCES") {
                ClosedCodeError::SandboxDenied {
                    command: command.to_string(),
                    reason: format!("Landlock denied operation (mode: {})", self.mode),
                }
            } else {
                ClosedCodeError::ShellError(format!(
                    "Landlock execution failed for '{}': {}",
                    command, e,
                ))
            }
        })?;

        Ok(output)
    }

    fn mode(&self) -> SandboxMode {
        self.mode
    }

    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Landlock
    }

    fn workspace(&self) -> &Path {
        &self.workspace
    }
}
```

**Key design decisions:**

| Decision | Rationale |
|----------|-----------|
| Landlock applied via `pre_exec`, not `restrict_self` on parent | Applying to parent would restrict our own file operations (reading files, writing diffs, etc.). `pre_exec` runs after `fork()` but before `exec()`, so only the child is restricted. |
| `WorkspaceOnly` enumerates allowed read paths | Restricting reads is the high-value security feature — prevents the LLM from reading `~/.ssh`, `~/.aws`, other projects. System paths are enumerated because commands need libraries/executables to run. |
| `/tmp` writable in both restricted modes | Many commands create temporary files. Blocking `/tmp` would break common tools like `git`, `cargo`, `rustc`. |
| seccomp BPF deferred | Landlock provides sufficient filesystem sandboxing. seccomp adds syscall-level filtering but requires extensive whitelisting that varies by distribution and would significantly increase complexity. It can be added incrementally later. |
| `unsafe` block for `pre_exec` | Required by the Rust `CommandExt` API. The closure is safe — it only calls Landlock APIs. |
| `path.exists()` check before adding rules | Linux system paths vary by distro (e.g., `/lib64` exists on Fedora but not Ubuntu). Skip non-existent paths gracefully. |

**Landlock access rules per mode:**

| Mode | Read access | Write access | Network |
|------|-------------|--------------|---------|
| `WorkspaceOnly` | Workspace + system essentials (`/usr`, `/bin`, `/etc`, `/dev`, `/proc`, `/tmp`, etc.) | Workspace + `/tmp` | Not restricted by Landlock (filesystem only) |
| `WorkspaceWrite` | `/` (all) | Workspace + `/tmp` | Not restricted by Landlock |

Note: Landlock is a filesystem-only sandbox. It cannot restrict network access. On macOS, Seatbelt handles network denial natively for `WorkspaceOnly` mode. On Linux, `WorkspaceOnly` blocks filesystem access outside the workspace but does not block network. Full network restriction on Linux would require seccomp or iptables, which are deferred.

---

## Sub-Phase 7.4: Fallback Backend

### New File: `src/sandbox/fallback.rs`

Used when no platform sandbox is available, or when `FullAccess` mode is selected.

```rust
use std::path::{Path, PathBuf};
use std::process::Output;

use async_trait::async_trait;
use tokio::process::Command;

use super::{Sandbox, SandboxBackend, SandboxMode};
use crate::error::{ClosedCodeError, Result};

#[derive(Debug)]
pub struct FallbackSandbox {
    mode: SandboxMode,
    workspace: PathBuf,
}

impl FallbackSandbox {
    pub fn new(mode: SandboxMode, workspace: PathBuf) -> Self {
        if mode != SandboxMode::FullAccess {
            tracing::warn!(
                "Sandboxing not available on this platform. \
                 Running in '{}' mode without OS-level restrictions. \
                 The command allowlist still applies.",
                mode,
            );
        }
        Self { mode, workspace }
    }
}

#[async_trait]
impl Sandbox for FallbackSandbox {
    async fn execute_command(
        &self,
        command: &str,
        args: &[String],
        cwd: &Path,
    ) -> Result<Output> {
        Command::new(command)
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| ClosedCodeError::ShellError(
                format!("Failed to execute '{}': {}", command, e),
            ))
    }

    fn mode(&self) -> SandboxMode {
        self.mode
    }

    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Fallback
    }

    fn workspace(&self) -> &Path {
        &self.workspace
    }
}
```

The fallback is intentionally simple. It executes commands directly via `tokio::process::Command`, identical to the current (pre-Phase 7) behavior. The only addition is a tracing warning when the mode is not `FullAccess`, alerting the user that they requested sandboxing but it isn't available.

---

## Sub-Phase 7.5: Shell Tool Integration

### `src/tool/shell.rs` — Modified

The `ShellCommandTool` gains an `Arc<dyn Sandbox>` field and routes all command execution through it.

**Updated struct:**

```rust
use std::sync::Arc;
use crate::sandbox::Sandbox;

#[derive(Debug)]
pub struct ShellCommandTool {
    working_directory: PathBuf,
    bypass_allowlist: bool,
    sandbox: Arc<dyn Sandbox>,
}
```

**Updated constructors:**

```rust
impl ShellCommandTool {
    pub fn new(working_directory: PathBuf, sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            working_directory,
            bypass_allowlist: false,
            sandbox,
        }
    }

    pub fn with_bypass_allowlist(working_directory: PathBuf, sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            working_directory,
            bypass_allowlist: true,
            sandbox,
        }
    }
}
```

**Updated `execute()` method:**

The key change is replacing the direct `Command::new()` call with `self.sandbox.execute_command()`:

```rust
#[async_trait]
impl Tool for ShellCommandTool {
    async fn execute(&self, args: Value) -> Result<Value> {
        let command_str = args["command"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "shell".into(),
                message: "Missing required parameter 'command'".into(),
            })?;

        let (cmd, cmd_args) = if self.bypass_allowlist {
            Self::parse_without_validation(command_str)?
        } else {
            Self::parse_and_validate(command_str)?
        };

        tracing::info!("Executing shell command: {} {:?}", cmd, cmd_args);

        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            self.sandbox.execute_command(&cmd, &cmd_args, &self.working_directory),
        )
        .await
        .map_err(|_| ClosedCodeError::ShellTimeout { seconds: 30 })??;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // Truncate very long output
        let max_output = 50_000;
        let stdout_truncated = if stdout.len() > max_output {
            format!(
                "{}...\n[Output truncated: {} bytes total]",
                &stdout[..max_output],
                stdout.len()
            )
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

    // name(), description(), declaration() unchanged
}
```

**Changes vs. current implementation:**

| Before (Phase 6) | After (Phase 7) |
|---|---|
| `Command::new(&cmd).args(&cmd_args).current_dir(...).output().await` | `self.sandbox.execute_command(&cmd, &cmd_args, &self.working_directory).await` |
| Timeout wraps `Command::new()...output()` | Timeout wraps `self.sandbox.execute_command()` |
| Error: `ClosedCodeError::ShellError(...)` | Error: `ClosedCodeError::SandboxDenied { .. }` or `ShellError(...)` |

The allowlist check (`parse_and_validate`) still happens **before** the sandbox. This is defense-in-depth: the allowlist is the first gate (application-level), the sandbox is the second gate (OS-level).

---

## Sub-Phase 7.6: Extended Protected Paths

### `src/tool/mod.rs` — Modified

The `is_protected_path()` function currently exists independently in both `file_write.rs` and `file_edit.rs`. It moves to `tool/mod.rs` as a shared, configurable function.

**New shared function:**

```rust
/// Default hardcoded protected paths.
const DEFAULT_PROTECTED_PATHS: &[&str] = &[
    ".git",
    ".closed-code",
    ".env",
];

/// Default protected file extensions (matched case-insensitively).
const DEFAULT_PROTECTED_EXTENSIONS: &[&str] = &[
    ".pem",
    ".key",
];

/// Check if a path is protected from modification.
///
/// A path is protected if it matches any of:
/// - Hardcoded paths: .git/, .closed-code/, .env
/// - Hardcoded extensions: *.pem, *.key
/// - User-configured additional protected paths from config.toml
pub fn is_protected_path(path: &str, additional_paths: &[String]) -> bool {
    let normalized = path.replace('\\', "/");

    // Check hardcoded directory/file paths
    for protected in DEFAULT_PROTECTED_PATHS {
        if normalized == *protected || normalized.starts_with(&format!("{protected}/")) {
            return true;
        }
    }

    // Check hardcoded extensions
    let lower = normalized.to_lowercase();
    for ext in DEFAULT_PROTECTED_EXTENSIONS {
        if lower.ends_with(ext) {
            return true;
        }
    }

    // Check user-configured additional paths
    for additional in additional_paths {
        let additional_normalized = additional.replace('\\', "/");
        if normalized == additional_normalized
            || normalized.starts_with(&format!("{additional_normalized}/"))
        {
            return true;
        }
    }

    false
}
```

**Protected paths summary:**

| Path / Pattern | Reason | Source |
|----------------|--------|--------|
| `.git/` | Git internal data — never modified by LLM | Hardcoded |
| `.closed-code/` | Application config | Hardcoded |
| `.env` | Environment secrets | Hardcoded |
| `*.pem` | TLS certificates / private keys | Hardcoded |
| `*.key` | Private key files | Hardcoded |
| User-configured | Project-specific sensitive files | `config.toml` |

### `src/tool/file_write.rs` and `src/tool/file_edit.rs` — Modified

Both files are updated to:

1. Remove their local `is_protected_path()` functions.
2. Import the shared version from `super::is_protected_path`.
3. Accept configurable additional paths.

**Updated structs:**

```rust
pub struct WriteFileTool {
    working_directory: PathBuf,
    approval_handler: Arc<dyn ApprovalHandler>,
    protected_paths: Vec<String>,   // NEW: configurable additional paths
}

pub struct EditFileTool {
    working_directory: PathBuf,
    approval_handler: Arc<dyn ApprovalHandler>,
    protected_paths: Vec<String>,   // NEW: configurable additional paths
}
```

**Updated constructors:**

```rust
impl WriteFileTool {
    pub fn new(
        working_directory: PathBuf,
        approval_handler: Arc<dyn ApprovalHandler>,
        protected_paths: Vec<String>,
    ) -> Self {
        Self {
            working_directory,
            approval_handler,
            protected_paths,
        }
    }
}

// Same pattern for EditFileTool
```

**Updated execute() call:**

```rust
// Before:
if is_protected_path(path_str) { ... }

// After:
if super::is_protected_path(path_str, &self.protected_paths) { ... }
```

---

## Sub-Phase 7.7: Config + CLI Changes

### `src/config.rs` — Modified

**New `SecurityConfig` struct for TOML:**

```rust
#[derive(Debug, Default, Deserialize)]
pub struct SecurityConfig {
    pub sandbox_mode: Option<String>,
    pub protected_paths: Option<Vec<String>>,
}
```

**Updated `TomlConfig`:**

```rust
#[derive(Debug, Default, Deserialize)]
pub struct TomlConfig {
    // ... existing fields ...
    #[serde(default)]
    pub security: Option<SecurityConfig>,
}
```

**Updated `Config` struct:**

```rust
#[derive(Debug, Clone)]
pub struct Config {
    // ... existing fields ...
    pub sandbox_mode: SandboxMode,
    pub protected_paths: Vec<String>,
}
```

**Updated `Config::from_cli()`:**

```rust
// Resolve sandbox_mode: CLI > TOML > default (WorkspaceWrite)
let sandbox_mode = if let Some(ref s) = cli.sandbox {
    s.parse::<SandboxMode>()?
} else if let Some(ref sec) = merged.security {
    if let Some(ref s) = sec.sandbox_mode {
        s.parse::<SandboxMode>()?
    } else {
        SandboxMode::default()
    }
} else {
    SandboxMode::default()
};

let protected_paths = merged
    .security
    .and_then(|s| s.protected_paths)
    .unwrap_or_default();
```

**Updated `Config::merge()`:**

```rust
fn merge(base: TomlConfig, overlay: TomlConfig) -> TomlConfig {
    TomlConfig {
        // ... existing merges ...
        security: overlay.security.or(base.security),
    }
}
```

**Example `config.toml`:**

```toml
model = "gemini-3.1-pro-preview"
default_mode = "execute"

[security]
sandbox_mode = "workspace-write"
protected_paths = [".secrets/", "credentials.json", "*.p12"]
```

### `src/cli.rs` — Modified

**New CLI flag:**

```rust
#[derive(Parser, Debug)]
#[command(name = "closed-code")]
pub struct Cli {
    // ... existing fields ...

    /// Sandbox mode: workspace-only, workspace-write, full-access
    #[arg(long, value_name = "MODE")]
    pub sandbox: Option<String>,
}
```

Usage: `cargo run -- --sandbox workspace-only`

### `src/error.rs` — Modified

**New error variants:**

```rust
#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // ... existing variants ...

    // Sandbox errors (Phase 7)
    #[error("Sandbox denied: command '{command}' — {reason}")]
    SandboxDenied { command: String, reason: String },

    #[error("Invalid sandbox mode '{0}'. Expected: workspace-only, workspace-write, full-access")]
    InvalidSandboxMode(String),
}
```

`SandboxDenied` is returned when the OS-level sandbox rejects a command. This is distinct from `ShellNotAllowed` (allowlist rejection) — the allowlist is checked first, then the sandbox.

---

## Sub-Phase 7.8: Integration + Slash Command

### `src/tool/registry.rs` — Modified

All factory functions gain a `sandbox` parameter that is threaded through to `ShellCommandTool`:

```rust
pub fn create_default_registry(
    working_dir: PathBuf,
    sandbox: Arc<dyn Sandbox>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool::new(working_dir.clone())));
    registry.register(Box::new(ListDirectoryTool::new(working_dir.clone())));
    registry.register(Box::new(SearchFilesTool::new(working_dir.clone())));
    registry.register(Box::new(GrepTool::new(working_dir.clone())));
    registry.register(Box::new(ShellCommandTool::new(working_dir, sandbox)));
    registry
}

pub fn create_subagent_registry(
    working_dir: PathBuf,
    sandbox: Arc<dyn Sandbox>,
) -> ToolRegistry {
    let mut registry = Self::create_default_registry(working_dir.clone(), sandbox);
    registry.register(Box::new(CreateReportTool));
    registry
}

pub fn create_orchestrator_registry(
    working_dir: PathBuf,
    mode: &Mode,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    client: Arc<GeminiClient>,
    sandbox: Arc<dyn Sandbox>,
    protected_paths: Vec<String>,
) -> ToolRegistry {
    let mut registry = Self::create_default_registry(working_dir.clone(), sandbox.clone());

    // For Auto mode, replace shell tool with bypass version
    if *mode == Mode::Auto {
        registry.register(Box::new(
            ShellCommandTool::with_bypass_allowlist(working_dir.clone(), sandbox),
        ));
    }

    // Register write tools if approval_handler is provided
    if let Some(handler) = approval_handler {
        registry.register(Box::new(WriteFileTool::new(
            working_dir.clone(),
            handler.clone(),
            protected_paths.clone(),
        )));
        registry.register(Box::new(EditFileTool::new(
            working_dir.clone(),
            handler,
            protected_paths,
        )));
    }

    // Register spawn tools
    // ... existing spawn tool registration ...

    registry
}
```

### `src/agent/orchestrator.rs` — Modified

**New field:**

```rust
pub struct Orchestrator {
    // ... existing fields ...
    // Phase 7
    sandbox: Arc<dyn Sandbox>,
    protected_paths: Vec<String>,
}
```

**Updated `new()`:**

The constructor accepts `sandbox: Arc<dyn Sandbox>` and `protected_paths: Vec<String>`, passes them to `create_orchestrator_registry()`.

**Updated `build_system_prompt()`:**

When sandbox is active (not FullAccess), append to the system prompt:

```
Sandbox: workspace-write (macOS Seatbelt)
  - File reads: everywhere. File writes: workspace directory only.
  - Network: allowed.
  - Protected paths: .git/, .closed-code/, .env, *.pem, *.key
```

Or for `WorkspaceOnly`:

```
Sandbox: workspace-only (macOS Seatbelt)
  - File reads: workspace + system paths only. File writes: workspace only.
  - Network: blocked.
  - Protected paths: .git/, .closed-code/, .env, *.pem, *.key
  - You cannot read files outside this project directory.
```

**New accessor methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `sandbox_mode()` | `&self -> SandboxMode` | Returns current sandbox mode |
| `sandbox_summary()` | `&self -> String` | One-line summary: `"workspace-write (macOS Seatbelt)"` |

### `src/repl.rs` — Modified

**Updated startup banner:**

```
closed-code
Mode: explore | Model: gemini-3.1-pro-preview | Tools: 9
Working directory: /Users/me/project
Sandbox: workspace-write (macOS Seatbelt)
Git: main (3 uncommitted changes)
Type /help for commands, Ctrl+C to interrupt, /quit to exit.
```

**New `/sandbox` slash command:**

```
> /sandbox
Sandbox mode: workspace-write
Backend: macOS Seatbelt
Workspace: /Users/me/project
Protected paths:
  .git/         (hardcoded)
  .closed-code/ (hardcoded)
  .env          (hardcoded)
  *.pem         (hardcoded)
  *.key         (hardcoded)
  .secrets/     (config.toml)
  credentials.json (config.toml)
```

The `/sandbox` command is read-only — it displays the current sandbox configuration. The sandbox mode cannot be changed at runtime (immutable for security).

**Updated `/help`:**

```
/sandbox          — Show sandbox mode and backend info
```

**Updated `/status`:**

```
Mode: explore | Model: gemini-3.1-pro-preview | Personality: pragmatic
Sandbox: workspace-write (macOS Seatbelt)
Git: main (3 uncommitted changes)
Tokens: 1,234 prompt + 567 completion = 1,801 total (3 API calls)
Turns: 4 / 50 | Tools: 9
```

### `src/main.rs` — Modified

Updated startup flow to create the sandbox before the orchestrator:

```rust
// After config is loaded, before orchestrator creation:
let sandbox = sandbox::create_sandbox(config.sandbox_mode, config.working_directory.clone());
```

The `sandbox` is then passed to `Orchestrator::new()` and through to the REPL/one-shot functions.

---

## Test Summary

| File | New Tests | Category |
|------|-----------|----------|
| `src/sandbox/mod.rs` | 8 | SandboxMode Display/FromStr, SandboxBackend Display, factory with FullAccess returns Fallback, default mode |
| `src/sandbox/macos.rs` | 6 | Profile generation (WorkspaceOnly, WorkspaceWrite), workspace path escaping, is_available check, resolve_command |
| `src/sandbox/linux.rs` | 4 | is_supported check, build_ruleset construction, mode behavior |
| `src/sandbox/fallback.rs` | 4 | Execute passthrough, mode/backend accessors, warning logged for non-FullAccess |
| `src/tool/shell.rs` | 6 | Sandbox routing (mock), SandboxDenied error propagation, existing tests updated with MockSandbox |
| `src/tool/mod.rs` | 12 | Shared is_protected_path: .git, .closed-code, .env, *.pem, *.key, custom paths, non-protected paths, edge cases |
| `src/tool/file_write.rs` | 4 | Updated to use shared is_protected_path, custom protected paths respected, *.pem blocked |
| `src/tool/file_edit.rs` | 3 | Updated to use shared is_protected_path, custom protected paths respected |
| `src/config.rs` | 5 | SecurityConfig TOML parsing, sandbox_mode resolution, protected_paths resolution, merge, defaults |
| `src/error.rs` | 3 | SandboxDenied display, InvalidSandboxMode display, not retryable |
| `src/repl.rs` | 3 | /sandbox command output, /status includes sandbox, /help includes /sandbox |
| **Total** | **58 new tests** | |

### MockSandbox for Testing

A `MockSandbox` is added to `src/sandbox/mod.rs` behind `#[cfg(test)]`:

```rust
#[cfg(test)]
pub mod mock {
    use super::*;

    /// A mock sandbox for testing that executes commands directly.
    #[derive(Debug)]
    pub struct MockSandbox {
        mode: SandboxMode,
        workspace: PathBuf,
    }

    impl MockSandbox {
        pub fn new(workspace: PathBuf) -> Self {
            Self {
                mode: SandboxMode::FullAccess,
                workspace,
            }
        }

        pub fn with_mode(mode: SandboxMode, workspace: PathBuf) -> Self {
            Self { mode, workspace }
        }
    }

    #[async_trait]
    impl Sandbox for MockSandbox {
        async fn execute_command(
            &self,
            command: &str,
            args: &[String],
            cwd: &Path,
        ) -> Result<Output> {
            // Delegate to real execution (like FallbackSandbox)
            tokio::process::Command::new(command)
                .args(args)
                .current_dir(cwd)
                .output()
                .await
                .map_err(|e| ClosedCodeError::ShellError(
                    format!("Mock execution failed: {}", e),
                ))
        }

        fn mode(&self) -> SandboxMode { self.mode }
        fn backend(&self) -> SandboxBackend { SandboxBackend::Fallback }
        fn workspace(&self) -> &Path { &self.workspace }
    }
}
```

All existing tests that construct `ShellCommandTool` or use the tool registry must be updated to pass an `Arc<MockSandbox>`. This is a straightforward search-and-replace since `MockSandbox::new()` delegates to real execution, preserving existing test behavior.

---

## Milestone

```bash
# Default behavior (workspace-write) — most users never think about sandboxing
cargo run -- --api-key $KEY
# closed-code
# Mode: explore | Model: gemini-3.1-pro-preview | Tools: 9
# Working directory: /Users/me/project
# Sandbox: workspace-write (macOS Seatbelt)
# Git: main (clean)

# Default mode allows reads everywhere, writes only in project, network allowed
# > Run git log --oneline -5
# ⠋ Using shell: git log --oneline -5...
# abc1234 Initial commit
# ✓ exit_code: 0

# > Run git push origin main
# ⠋ Using shell: git push origin main...
# ✓ (network allowed in workspace-write mode)

# > Run cargo build
# ⠋ Using shell: cargo build...
# ✓ (fetches crates over network, writes to ./target — both allowed)

# Write outside workspace is blocked
# > (in workspace-write mode) Run touch /etc/test.txt
# ✗ Sandbox denied: command 'touch' — Seatbelt denied operation (mode: workspace-write)

# Write within workspace succeeds
# > (in workspace-write mode) Run touch ./test.txt
# ✓ exit_code: 0

# Workspace-only mode — strongest isolation
cargo run -- --api-key $KEY --sandbox workspace-only
# Sandbox: workspace-only (macOS Seatbelt)

# Cannot read files outside the project
# > Run cat ~/.ssh/id_rsa
# ✗ Sandbox denied: command 'cat' — Seatbelt denied operation (mode: workspace-only)

# Cannot read other projects
# > Run ls ~/other-project/
# ✗ Sandbox denied: command 'ls' — Seatbelt denied operation (mode: workspace-only)

# Network is blocked
# > Run git push origin main
# ✗ Sandbox denied: command 'git' — Seatbelt denied operation (mode: workspace-only)

# But workspace reads/writes and system commands work fine
# > Run git log --oneline -5
# ✓ exit_code: 0 (reads git objects within workspace)

# > Run ls src/
# ✓ main.rs  lib.rs  ...

# Full access (no sandbox)
cargo run -- --api-key $KEY --sandbox full-access
# Sandbox: full-access (fallback)
# (no restrictions, identical to pre-Phase 7 behavior)

# Protected paths (extended) — enforced regardless of sandbox mode
# > (in execute mode) Edit .env
# Error: Cannot modify protected path: .env

# > (in execute mode) Edit secrets/api.pem
# Error: Cannot modify protected path: secrets/api.pem

# /sandbox command
# > /sandbox
# Sandbox mode: workspace-write
# Backend: macOS Seatbelt
# Workspace: /Users/me/project
# Protected paths:
#   .git/         (hardcoded)
#   .closed-code/ (hardcoded)
#   .env          (hardcoded)
#   *.pem         (hardcoded)
#   *.key         (hardcoded)

# /status includes sandbox
# > /status
# Mode: explore | Model: gemini-3.1-pro-preview | Personality: pragmatic
# Sandbox: workspace-write (macOS Seatbelt)
# Git: main (clean)
# Tokens: 1,234 prompt + 567 completion = 1,801 total (3 API calls)
# Turns: 4 / 50 | Tools: 9

# Config file
# ~/.closed-code/config.toml:
# [security]
# sandbox_mode = "workspace-only"
# protected_paths = [".secrets/", "credentials.json"]

# Linux sandbox
# (on Linux with Landlock-enabled kernel)
cargo run -- --api-key $KEY
# Sandbox: workspace-write (Linux Landlock)

# Unsupported platform / old kernel
# (falls back gracefully)
cargo run -- --api-key $KEY --sandbox workspace-only
# Warning: Sandboxing not available on this platform. Running without OS-level restrictions.
# Sandbox: workspace-only (fallback)

# Tests
cargo test
# running 393 tests (335 existing + 58 new)
# test sandbox::tests::... ok
# test tool::tests::is_protected_path_... ok
# ...
# test result: ok. 393 passed; 0 failed
```

---

## Implementation Order

1. `src/sandbox/mod.rs` — `SandboxMode`, `SandboxBackend`, `Sandbox` trait, `create_sandbox()` factory, `MockSandbox`
2. `src/sandbox/fallback.rs` — `FallbackSandbox` (simplest backend)
3. `src/sandbox/macos.rs` — `SeatbeltSandbox` with profile generation (macOS only)
4. `src/sandbox/linux.rs` — `LandlockSandbox` with `pre_exec` hook (Linux only)
5. `src/lib.rs` — add `pub mod sandbox;`
6. `cargo test` checkpoint — sandbox module unit tests pass
7. `src/error.rs` — add `SandboxDenied`, `InvalidSandboxMode` variants
8. `src/tool/mod.rs` — shared `is_protected_path()` with configurable paths
9. `src/tool/file_write.rs` + `src/tool/file_edit.rs` — remove local `is_protected_path()`, use shared version, add `protected_paths` field
10. `cargo test` checkpoint — protected path tests pass
11. `src/tool/shell.rs` — add `sandbox: Arc<dyn Sandbox>`, route execution through it, update constructors
12. `src/tool/registry.rs` — thread `sandbox` and `protected_paths` through all factory functions
13. `cargo test` checkpoint — shell + registry tests pass (all existing tests updated with MockSandbox)
14. `src/config.rs` — add `SecurityConfig`, `sandbox_mode`, `protected_paths` to Config
15. `src/cli.rs` — add `--sandbox` flag
16. `src/agent/orchestrator.rs` — add `sandbox` field, pass through to registry, sandbox info in system prompt, new accessors
17. `src/repl.rs` — `/sandbox` command, updated startup banner, updated `/help` and `/status`
18. `src/main.rs` — create sandbox from config, pass to orchestrator
19. `cargo test` — all 393 tests pass (335 existing + 58 new)

---

## Complexity: **High**

Platform-specific code with conditional compilation (`#[cfg(target_os = ...)]`), Seatbelt profile string generation, Landlock FFI via the `landlock` crate with `unsafe pre_exec`, and significant plumbing to thread `Arc<dyn Sandbox>` through the tool registry and orchestrator. The `MockSandbox` pattern keeps existing tests working with minimal changes. ~4 new files, ~11 modified files, ~58 new tests, ~2,000 lines.

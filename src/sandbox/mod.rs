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
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{ClosedCodeError, Result};

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

impl fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceOnly => write!(f, "workspace-only"),
            Self::WorkspaceWrite => write!(f, "workspace-write"),
            Self::FullAccess => write!(f, "full-access"),
        }
    }
}

impl std::str::FromStr for SandboxMode {
    type Err = ClosedCodeError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "workspace-only" | "workspace_only" => Ok(Self::WorkspaceOnly),
            "workspace-write" | "workspace_write" => Ok(Self::WorkspaceWrite),
            "full-access" | "full_access" => Ok(Self::FullAccess),
            _ => Err(ClosedCodeError::InvalidSandboxMode(s.to_string())),
        }
    }
}

/// Identifies which platform sandbox backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    Seatbelt, // macOS sandbox-exec
    Landlock, // Linux Landlock LSM
    Fallback, // No OS-level sandbox
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

#[async_trait]
pub trait Sandbox: Send + Sync + fmt::Debug {
    /// Execute a command with sandbox restrictions applied.
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

    /// The workspace directory that restricted modes permit writes to.
    fn workspace(&self) -> &Path;
}

/// Create the appropriate sandbox for the current platform and mode.
///
/// - FullAccess always returns FallbackSandbox (no restrictions needed).
/// - macOS: returns SeatbeltSandbox if sandbox-exec is available.
/// - Linux: returns LandlockSandbox if Landlock is supported by the kernel.
/// - Otherwise: returns FallbackSandbox with a tracing warning.
pub fn create_sandbox(mode: SandboxMode, workspace: PathBuf) -> Arc<dyn Sandbox> {
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
            tokio::process::Command::new(command)
                .args(args)
                .current_dir(cwd)
                .output()
                .await
                .map_err(|e| {
                    ClosedCodeError::ShellError(format!("Mock execution failed: {}", e))
                })
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_mode_default_is_workspace_write() {
        assert_eq!(SandboxMode::default(), SandboxMode::WorkspaceWrite);
    }

    #[test]
    fn sandbox_mode_display() {
        assert_eq!(SandboxMode::WorkspaceOnly.to_string(), "workspace-only");
        assert_eq!(SandboxMode::WorkspaceWrite.to_string(), "workspace-write");
        assert_eq!(SandboxMode::FullAccess.to_string(), "full-access");
    }

    #[test]
    fn sandbox_mode_from_str_valid() {
        assert_eq!(
            "workspace-only".parse::<SandboxMode>().unwrap(),
            SandboxMode::WorkspaceOnly
        );
        assert_eq!(
            "workspace_only".parse::<SandboxMode>().unwrap(),
            SandboxMode::WorkspaceOnly
        );
        assert_eq!(
            "workspace-write".parse::<SandboxMode>().unwrap(),
            SandboxMode::WorkspaceWrite
        );
        assert_eq!(
            "workspace_write".parse::<SandboxMode>().unwrap(),
            SandboxMode::WorkspaceWrite
        );
        assert_eq!(
            "full-access".parse::<SandboxMode>().unwrap(),
            SandboxMode::FullAccess
        );
        assert_eq!(
            "full_access".parse::<SandboxMode>().unwrap(),
            SandboxMode::FullAccess
        );
    }

    #[test]
    fn sandbox_mode_from_str_invalid() {
        let result = "bad".parse::<SandboxMode>();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::InvalidSandboxMode(_)
        ));
    }

    #[test]
    fn sandbox_backend_display() {
        assert_eq!(SandboxBackend::Seatbelt.to_string(), "macOS Seatbelt");
        assert_eq!(SandboxBackend::Landlock.to_string(), "Linux Landlock");
        assert_eq!(
            SandboxBackend::Fallback.to_string(),
            "fallback (unsandboxed)"
        );
    }

    #[test]
    fn create_sandbox_full_access_returns_fallback() {
        let sandbox = create_sandbox(SandboxMode::FullAccess, PathBuf::from("/tmp"));
        assert_eq!(sandbox.mode(), SandboxMode::FullAccess);
        assert_eq!(sandbox.backend(), SandboxBackend::Fallback);
    }

    #[test]
    fn mock_sandbox_mode_and_backend() {
        let sandbox = mock::MockSandbox::new(PathBuf::from("/tmp"));
        assert_eq!(sandbox.mode(), SandboxMode::FullAccess);
        assert_eq!(sandbox.backend(), SandboxBackend::Fallback);
    }

    #[test]
    fn mock_sandbox_with_mode() {
        let sandbox =
            mock::MockSandbox::with_mode(SandboxMode::WorkspaceOnly, PathBuf::from("/project"));
        assert_eq!(sandbox.mode(), SandboxMode::WorkspaceOnly);
        assert_eq!(sandbox.workspace(), Path::new("/project"));
    }
}

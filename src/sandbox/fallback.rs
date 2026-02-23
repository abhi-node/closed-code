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
    async fn execute_command(&self, command: &str, args: &[String], cwd: &Path) -> Result<Output> {
        Command::new(command)
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| {
                ClosedCodeError::ShellError(format!("Failed to execute '{}': {}", command, e))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fallback_execute_echo() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = FallbackSandbox::new(SandboxMode::FullAccess, dir.path().to_path_buf());
        let output = sandbox
            .execute_command("echo", &["hello".to_string()], dir.path())
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    #[test]
    fn fallback_mode_accessor() {
        let sandbox = FallbackSandbox::new(SandboxMode::FullAccess, PathBuf::from("/tmp"));
        assert_eq!(sandbox.mode(), SandboxMode::FullAccess);
    }

    #[test]
    fn fallback_backend_is_fallback() {
        let sandbox = FallbackSandbox::new(SandboxMode::FullAccess, PathBuf::from("/tmp"));
        assert_eq!(sandbox.backend(), SandboxBackend::Fallback);
    }

    #[test]
    fn fallback_workspace_accessor() {
        let sandbox = FallbackSandbox::new(SandboxMode::FullAccess, PathBuf::from("/my/project"));
        assert_eq!(sandbox.workspace(), Path::new("/my/project"));
    }
}

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
            .map_err(|e| {
                ClosedCodeError::ShellError(format!(
                    "Failed to resolve command '{}': {}",
                    command, e
                ))
            })?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(path)
        } else {
            // Fall back to the bare command name
            Ok(command.to_string())
        }
    }
}

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
            .map_err(|e| {
                ClosedCodeError::ShellError(format!(
                    "Seatbelt execution failed for '{}': {}",
                    command, e
                ))
            })?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seatbelt_workspace_only_profile_contains_deny_network() {
        let profile =
            SeatbeltSandbox::workspace_only_profile(Path::new("/Users/me/project"));
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn seatbelt_workspace_write_profile_allows_network() {
        let profile =
            SeatbeltSandbox::workspace_write_profile(Path::new("/Users/me/project"));
        assert!(profile.contains("(allow network*)"));
        assert!(!profile.contains("(deny network*)"));
    }

    #[test]
    fn seatbelt_workspace_only_profile_restricts_reads() {
        let profile =
            SeatbeltSandbox::workspace_only_profile(Path::new("/Users/me/project"));
        // Should NOT have a blanket allow file-read*
        // Should have specific subpath reads
        assert!(profile.contains(r#"(allow file-read* (subpath "/Users/me/project"))"#));
        assert!(profile.contains(r#"(allow file-read* (subpath "/usr"))"#));
        assert!(!profile.contains("(allow file-read*)\n"));
    }

    #[test]
    fn seatbelt_workspace_write_profile_allows_reads() {
        let profile =
            SeatbeltSandbox::workspace_write_profile(Path::new("/Users/me/project"));
        assert!(profile.contains("(allow file-read*)"));
    }

    #[test]
    fn seatbelt_profile_includes_workspace_path() {
        let profile =
            SeatbeltSandbox::workspace_only_profile(Path::new("/my/custom/path"));
        assert!(profile.contains("/my/custom/path"));
    }

    #[test]
    fn resolve_command_absolute_path_passthrough() {
        let result = SeatbeltSandbox::resolve_command("/usr/bin/git").unwrap();
        assert_eq!(result, "/usr/bin/git");
    }
}

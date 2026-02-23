use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use async_trait::async_trait;
use landlock::{
    Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
    ABI,
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

impl LandlockSandbox {
    pub fn new(mode: SandboxMode, workspace: PathBuf) -> Self {
        Self { mode, workspace }
    }

    /// Check if the running kernel supports Landlock.
    pub fn is_supported() -> bool {
        Ruleset::default()
            .handle_access(AccessFs::from_all(MIN_ABI))
            .is_ok()
    }

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
                                    PathFd::new(path).map_err(|e| {
                                        std::io::Error::new(std::io::ErrorKind::Other, e)
                                    })?,
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
                eprintln!("Warning: Landlock ruleset was not enforced by kernel");
            }

            Ok(())
        }
    }
}

#[async_trait]
impl Sandbox for LandlockSandbox {
    async fn execute_command(&self, command: &str, args: &[String], cwd: &Path) -> Result<Output> {
        let mut ruleset_fn = self.build_ruleset();

        // SAFETY: pre_exec runs after fork() but before exec() in the child process.
        // This applies Landlock restrictions only to the child, not our process.
        // The closure only calls Landlock APIs which are safe.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landlock_mode_accessor() {
        let sandbox = LandlockSandbox::new(SandboxMode::WorkspaceWrite, PathBuf::from("/project"));
        assert_eq!(sandbox.mode(), SandboxMode::WorkspaceWrite);
    }

    #[test]
    fn landlock_backend_is_landlock() {
        let sandbox = LandlockSandbox::new(SandboxMode::WorkspaceWrite, PathBuf::from("/project"));
        assert_eq!(sandbox.backend(), SandboxBackend::Landlock);
    }

    #[test]
    fn landlock_workspace_accessor() {
        let sandbox =
            LandlockSandbox::new(SandboxMode::WorkspaceWrite, PathBuf::from("/my/project"));
        assert_eq!(sandbox.workspace(), Path::new("/my/project"));
    }

    #[test]
    fn landlock_is_supported_does_not_panic() {
        // This test verifies the check doesn't panic; actual support depends on kernel
        let _ = LandlockSandbox::is_supported();
    }
}

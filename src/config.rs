use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::cli::Cli;
use crate::error::ClosedCodeError;
use crate::mode::Mode;
use crate::sandbox::SandboxMode;

// ── Personality ──

/// Personality style that modifies the system prompt prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Personality {
    /// Warm, encouraging, casual language.
    Friendly,
    /// Direct, concise, code-focused.
    #[default]
    Pragmatic,
    /// Minimal personality, just answers.
    None,
}

impl fmt::Display for Personality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Friendly => write!(f, "friendly"),
            Self::Pragmatic => write!(f, "pragmatic"),
            Self::None => write!(f, "none"),
        }
    }
}

impl FromStr for Personality {
    type Err = ClosedCodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "friendly" => Ok(Self::Friendly),
            "pragmatic" => Ok(Self::Pragmatic),
            "none" => Ok(Self::None),
            _ => Err(ClosedCodeError::InvalidPersonality(s.to_string())),
        }
    }
}

// ── TOML Config (intermediate, all fields optional for layered merging) ──

#[derive(Debug, Default, Deserialize)]
pub struct TomlConfig {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub default_mode: Option<String>,
    pub personality: Option<String>,
    pub context_limit_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub verbose: Option<bool>,
    #[serde(default)]
    pub shell: Option<ShellConfig>,
    #[serde(default)]
    pub security: Option<SecurityConfig>,
    #[serde(default)]
    pub session: Option<SessionConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ShellConfig {
    pub additional_allowlist: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SecurityConfig {
    pub sandbox_mode: Option<String>,
    pub protected_paths: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SessionConfig {
    pub auto_save: Option<bool>,
    pub transcript_logging: Option<bool>,
    pub sessions_dir: Option<String>,
}

// ── Final Config ──

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub model: String,
    pub mode: Mode,
    pub working_directory: PathBuf,
    pub personality: Personality,
    pub shell_additional_allowlist: Vec<String>,
    pub context_limit_tokens: u32,
    pub verbose: bool,
    pub max_output_tokens: u32,
    pub sandbox_mode: SandboxMode,
    pub protected_paths: Vec<String>,
    // Phase 8a
    pub session_auto_save: bool,
    pub session_transcript_logging: bool,
    pub sessions_dir: PathBuf,
}

impl Config {
    /// Build final Config from CLI args, layered TOML files, and env vars.
    ///
    /// Resolution order (later wins):
    ///   hardcoded defaults → ~/.closed-code/config.toml → <working_dir>/.closed-code/config.toml → env → CLI
    pub fn from_cli(cli: &Cli) -> crate::error::Result<Self> {
        // Determine working directory early (needed for project config path)
        let working_directory = cli
            .directory
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        // Load and merge TOML layers
        let user_toml = Self::load_user_config()?;
        let project_toml = Self::load_project_config(&working_directory)?;

        let merged = match (user_toml, project_toml) {
            (Some(base), Some(overlay)) => Self::merge(base, overlay),
            (Some(config), None) | (None, Some(config)) => config,
            (None, None) => TomlConfig::default(),
        };

        // Resolve each field: TOML merged → env → CLI (CLI wins)
        let api_key = cli
            .api_key
            .clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .or(merged.api_key)
            .ok_or(ClosedCodeError::MissingApiKey)?;

        let model = cli.model.clone();
        // If user didn't pass --model explicitly, check TOML
        // clap gives the default_value, so we can't easily distinguish.
        // Workaround: if TOML has model and CLI has the default, use TOML.
        // For simplicity, CLI always wins since clap provides a default.
        let model = merged.model.unwrap_or(model);
        // But CLI flag should override TOML — re-check if user explicitly passed --model
        // clap doesn't distinguish "default" vs "explicit". We use a simple heuristic:
        // if the cli.model != default, the user explicitly set it.
        let model = if cli.model != "gemini-3.1-pro-preview" {
            cli.model.clone()
        } else {
            model
        };

        let mode_str = if cli.mode != "explore" {
            cli.mode.clone()
        } else {
            merged.default_mode.unwrap_or_else(|| cli.mode.clone())
        };
        let mode = mode_str.parse::<Mode>()?;

        let personality = if let Some(ref p) = cli.personality {
            p.parse::<Personality>()?
        } else if let Some(ref p) = merged.personality {
            p.parse::<Personality>()?
        } else {
            Personality::default()
        };

        let context_limit_tokens = merged.context_limit_tokens.unwrap_or(1_000_000);

        let max_output_tokens = cli
            .max_output_tokens
            .or(merged.max_output_tokens)
            .unwrap_or(8192);

        let verbose = cli.verbose || merged.verbose.unwrap_or(false);

        let shell_additional_allowlist = merged
            .shell
            .and_then(|s| s.additional_allowlist)
            .unwrap_or_default();

        // Resolve sandbox mode: CLI → TOML → default (WorkspaceWrite)
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

        let session_auto_save = merged
            .session
            .as_ref()
            .and_then(|s| s.auto_save)
            .unwrap_or(true);
        let session_transcript_logging = merged
            .session
            .as_ref()
            .and_then(|s| s.transcript_logging)
            .unwrap_or(false);
        let sessions_dir = merged
            .session
            .as_ref()
            .and_then(|s| s.sessions_dir.as_ref())
            .map(PathBuf::from)
            .unwrap_or_else(crate::session::store::SessionStore::default_dir);

        Ok(Self {
            api_key,
            model,
            mode,
            working_directory,
            personality,
            shell_additional_allowlist,
            context_limit_tokens,
            verbose,
            max_output_tokens,
            sandbox_mode,
            protected_paths,
            session_auto_save,
            session_transcript_logging,
            sessions_dir,
        })
    }

    /// Load and parse a TOML file, returning None if it doesn't exist.
    fn load_toml_file(path: &Path) -> crate::error::Result<Option<TomlConfig>> {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let config: TomlConfig = toml::from_str(&contents).map_err(|e| {
                    ClosedCodeError::ConfigError(format!("{}: {}", path.display(), e))
                })?;
                Ok(Some(config))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ClosedCodeError::ConfigError(format!(
                "Failed to read {}: {}",
                path.display(),
                e
            ))),
        }
    }

    /// Merge two TomlConfig layers (overlay wins for present fields).
    fn merge(base: TomlConfig, overlay: TomlConfig) -> TomlConfig {
        TomlConfig {
            api_key: overlay.api_key.or(base.api_key),
            model: overlay.model.or(base.model),
            default_mode: overlay.default_mode.or(base.default_mode),
            personality: overlay.personality.or(base.personality),
            context_limit_tokens: overlay.context_limit_tokens.or(base.context_limit_tokens),
            max_output_tokens: overlay.max_output_tokens.or(base.max_output_tokens),
            verbose: overlay.verbose.or(base.verbose),
            shell: overlay.shell.or(base.shell),
            security: overlay.security.or(base.security),
            session: overlay.session.or(base.session),
        }
    }

    /// Load user config from ~/.closed-code/config.toml.
    fn load_user_config() -> crate::error::Result<Option<TomlConfig>> {
        if let Some(home) = dirs::home_dir() {
            let path = home.join(".closed-code").join("config.toml");
            Self::load_toml_file(&path)
        } else {
            Ok(None)
        }
    }

    /// Load project config from <working_dir>/.closed-code/config.toml.
    fn load_project_config(working_dir: &Path) -> crate::error::Result<Option<TomlConfig>> {
        let path = working_dir.join(".closed-code").join("config.toml");
        Self::load_toml_file(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn config_from_cli_defaults() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "test-key"]);
        let config = Config::from_cli(&cli).unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.mode, Mode::Explore);
        assert_eq!(config.personality, Personality::Pragmatic);
        assert_eq!(config.max_output_tokens, 8192);
        assert!(!config.verbose);
        assert!(config.shell_additional_allowlist.is_empty());
        assert_eq!(config.sandbox_mode, SandboxMode::WorkspaceWrite);
        assert!(config.protected_paths.is_empty());
    }

    #[test]
    fn config_from_cli_missing_api_key() {
        std::env::remove_var("GEMINI_API_KEY");
        let cli = Cli::parse_from(["closed-code"]);
        let result = Config::from_cli(&cli);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::MissingApiKey
        ));
    }

    #[test]
    fn config_from_cli_with_mode() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "k", "--mode", "execute"]);
        let config = Config::from_cli(&cli).unwrap();
        assert_eq!(config.mode, Mode::Execute);
    }

    #[test]
    fn config_from_cli_invalid_mode() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "k", "--mode", "bad"]);
        let result = Config::from_cli(&cli);
        assert!(result.is_err());
    }

    #[test]
    fn config_from_cli_with_directory() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "k", "-d", "/tmp"]);
        let config = Config::from_cli(&cli).unwrap();
        assert_eq!(config.working_directory, PathBuf::from("/tmp"));
    }

    #[test]
    fn config_from_cli_verbose() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "k", "-v"]);
        let config = Config::from_cli(&cli).unwrap();
        assert!(config.verbose);
    }

    #[test]
    fn config_from_cli_with_new_flags() {
        let cli = Cli::parse_from([
            "closed-code",
            "--api-key",
            "k",
            "--personality",
            "friendly",
            "--max-output-tokens",
            "4096",
        ]);
        let config = Config::from_cli(&cli).unwrap();
        assert_eq!(config.personality, Personality::Friendly);
        assert_eq!(config.max_output_tokens, 4096);
    }

    // ── TOML parsing ──

    #[test]
    fn config_toml_parsing() {
        let toml_str = r#"
api_key = "toml-key"
model = "gemini-2.0-flash"
default_mode = "plan"
personality = "friendly"
max_output_tokens = 4096
verbose = true

[shell]
additional_allowlist = ["docker", "cargo"]
"#;
        let config: TomlConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.api_key.as_deref(), Some("toml-key"));
        assert_eq!(config.model.as_deref(), Some("gemini-2.0-flash"));
        assert_eq!(config.default_mode.as_deref(), Some("plan"));
        assert_eq!(config.personality.as_deref(), Some("friendly"));
        assert_eq!(config.max_output_tokens, Some(4096));
        assert_eq!(config.verbose, Some(true));
        let shell = config.shell.unwrap();
        assert_eq!(shell.additional_allowlist.unwrap(), vec!["docker", "cargo"]);
    }

    #[test]
    fn config_toml_empty() {
        let config: TomlConfig = toml::from_str("").unwrap();
        assert!(config.api_key.is_none());
        assert!(config.model.is_none());
    }

    // ── Merge ──

    #[test]
    fn config_merge_overlay_wins() {
        let base = TomlConfig {
            model: Some("base-model".into()),
            personality: Some("pragmatic".into()),
            ..Default::default()
        };
        let overlay = TomlConfig {
            model: Some("overlay-model".into()),
            ..Default::default()
        };
        let merged = Config::merge(base, overlay);
        assert_eq!(merged.model.as_deref(), Some("overlay-model"));
        assert_eq!(merged.personality.as_deref(), Some("pragmatic"));
    }

    #[test]
    fn config_merge_none_preserves_base() {
        let base = TomlConfig {
            model: Some("base-model".into()),
            personality: Some("friendly".into()),
            ..Default::default()
        };
        let overlay = TomlConfig::default();
        let merged = Config::merge(base, overlay);
        assert_eq!(merged.model.as_deref(), Some("base-model"));
        assert_eq!(merged.personality.as_deref(), Some("friendly"));
    }

    // ── Personality ──

    #[test]
    fn personality_from_str() {
        assert_eq!(
            "friendly".parse::<Personality>().unwrap(),
            Personality::Friendly
        );
        assert_eq!(
            "pragmatic".parse::<Personality>().unwrap(),
            Personality::Pragmatic
        );
        assert_eq!("none".parse::<Personality>().unwrap(), Personality::None);
        assert_eq!(
            "FRIENDLY".parse::<Personality>().unwrap(),
            Personality::Friendly
        );
    }

    #[test]
    fn personality_from_str_invalid() {
        let result = "bad".parse::<Personality>();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::InvalidPersonality(_)
        ));
    }

    #[test]
    fn personality_default() {
        assert_eq!(Personality::default(), Personality::Pragmatic);
    }

    #[test]
    fn personality_display() {
        assert_eq!(Personality::Friendly.to_string(), "friendly");
        assert_eq!(Personality::Pragmatic.to_string(), "pragmatic");
        assert_eq!(Personality::None.to_string(), "none");
    }

    // ── Sandbox / Security Config ──

    #[test]
    fn config_from_cli_with_sandbox() {
        let cli = Cli::parse_from([
            "closed-code",
            "--api-key",
            "k",
            "--sandbox",
            "workspace-only",
        ]);
        let config = Config::from_cli(&cli).unwrap();
        assert_eq!(config.sandbox_mode, SandboxMode::WorkspaceOnly);
    }

    #[test]
    fn config_from_cli_invalid_sandbox() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "k", "--sandbox", "bad"]);
        let result = Config::from_cli(&cli);
        assert!(result.is_err());
    }

    #[test]
    fn config_toml_security_section() {
        let toml_str = r#"
api_key = "key"

[security]
sandbox_mode = "full-access"
protected_paths = ["secrets/", "credentials.json"]
"#;
        let config: TomlConfig = toml::from_str(toml_str).unwrap();
        let sec = config.security.unwrap();
        assert_eq!(sec.sandbox_mode.as_deref(), Some("full-access"));
        assert_eq!(
            sec.protected_paths.unwrap(),
            vec!["secrets/", "credentials.json"]
        );
    }

    #[test]
    fn config_merge_security() {
        let base = TomlConfig {
            security: Some(SecurityConfig {
                sandbox_mode: Some("workspace-only".into()),
                protected_paths: Some(vec!["base.key".into()]),
            }),
            ..Default::default()
        };
        let overlay = TomlConfig {
            security: Some(SecurityConfig {
                sandbox_mode: Some("full-access".into()),
                protected_paths: Some(vec!["overlay.key".into()]),
            }),
            ..Default::default()
        };
        let merged = Config::merge(base, overlay);
        let sec = merged.security.unwrap();
        assert_eq!(sec.sandbox_mode.as_deref(), Some("full-access"));
    }

    // ── Session Config Tests ──

    #[test]
    fn config_toml_session_section() {
        let toml_str = r#"
api_key = "key"

[session]
auto_save = false
transcript_logging = true
sessions_dir = "/tmp/my-sessions"
"#;
        let config: TomlConfig = toml::from_str(toml_str).unwrap();
        let sess = config.session.unwrap();
        assert_eq!(sess.auto_save, Some(false));
        assert_eq!(sess.transcript_logging, Some(true));
        assert_eq!(sess.sessions_dir.as_deref(), Some("/tmp/my-sessions"));
    }

    #[test]
    fn config_toml_session_empty() {
        let config: TomlConfig = toml::from_str("").unwrap();
        assert!(config.session.is_none());
    }

    #[test]
    fn config_session_defaults() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "test-key"]);
        let config = Config::from_cli(&cli).unwrap();
        assert!(config.session_auto_save); // default true
        assert!(!config.session_transcript_logging); // default false
        assert!(config.sessions_dir.to_string_lossy().contains("sessions"));
    }

    #[test]
    fn config_merge_session() {
        let base = TomlConfig {
            session: Some(SessionConfig {
                auto_save: Some(true),
                transcript_logging: Some(false),
                sessions_dir: Some("/base/sessions".into()),
            }),
            ..Default::default()
        };
        let overlay = TomlConfig {
            session: Some(SessionConfig {
                auto_save: Some(false),
                transcript_logging: Some(true),
                sessions_dir: None,
            }),
            ..Default::default()
        };
        let merged = Config::merge(base, overlay);
        let sess = merged.session.unwrap();
        assert_eq!(sess.auto_save, Some(false)); // overlay wins
        assert_eq!(sess.transcript_logging, Some(true));
    }
}

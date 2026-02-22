use std::path::PathBuf;

use crate::cli::Cli;
use crate::error::ClosedCodeError;
use crate::mode::Mode;

#[derive(Debug)]
pub struct Config {
    pub api_key: String,
    pub model: String,
    pub mode: Mode,
    pub working_directory: PathBuf,
    pub verbose: bool,
    pub max_output_tokens: u32,
}

impl Config {
    pub fn from_cli(cli: &Cli) -> crate::error::Result<Self> {
        let api_key = cli
            .api_key
            .clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .ok_or(ClosedCodeError::MissingApiKey)?;

        let mode = cli.mode.parse::<Mode>()?;

        let working_directory = cli
            .directory
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        Ok(Self {
            api_key,
            model: cli.model.clone(),
            mode,
            working_directory,
            verbose: cli.verbose,
            max_output_tokens: 8192,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn config_from_cli_with_api_key() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "test-key"]);
        let config = Config::from_cli(&cli).unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.model, "gemini-3.1-pro-preview");
        assert_eq!(config.mode, Mode::Explore);
        assert_eq!(config.max_output_tokens, 8192);
    }

    #[test]
    fn config_from_cli_missing_api_key() {
        // Unset the env var for this test
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
}

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "closed-code", version, about = "AI-powered coding CLI")]
pub struct Cli {
    /// Operating mode: explore, plan, or execute
    #[arg(long, default_value = "explore")]
    pub mode: String,

    /// Working directory (defaults to current dir)
    #[arg(long, short = 'd')]
    pub directory: Option<PathBuf>,

    /// Gemini API key (or set GEMINI_API_KEY env var)
    #[arg(long, env = "GEMINI_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    /// Model name
    #[arg(long, default_value = "gemini-3.1-pro-preview")]
    pub model: String,

    /// Enable verbose/debug output
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Send a one-shot question (non-interactive)
    Ask {
        /// The question to ask
        question: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_defaults() {
        let cli = Cli::parse_from(["closed-code"]);
        assert_eq!(cli.mode, "explore");
        assert_eq!(cli.model, "gemini-3.1-pro-preview");
        assert!(!cli.verbose);
        assert!(cli.directory.is_none());
        assert!(cli.command.is_none());
    }

    #[test]
    fn parse_mode_flag() {
        let cli = Cli::parse_from(["closed-code", "--mode", "plan"]);
        assert_eq!(cli.mode, "plan");
    }

    #[test]
    fn parse_directory_flag() {
        let cli = Cli::parse_from(["closed-code", "-d", "/tmp/project"]);
        assert_eq!(cli.directory.unwrap().to_str().unwrap(), "/tmp/project");
    }

    #[test]
    fn parse_verbose_flag() {
        let cli = Cli::parse_from(["closed-code", "-v"]);
        assert!(cli.verbose);
    }

    #[test]
    fn parse_ask_subcommand() {
        let cli = Cli::parse_from(["closed-code", "ask", "What is Rust?"]);
        match cli.command {
            Some(Commands::Ask { question }) => assert_eq!(question, "What is Rust?"),
            None => panic!("Expected Ask command"),
        }
    }

    #[test]
    fn parse_model_flag() {
        let cli = Cli::parse_from(["closed-code", "--model", "gemini-2.0-flash"]);
        assert_eq!(cli.model, "gemini-2.0-flash");
    }

    #[test]
    fn parse_api_key_flag() {
        let cli = Cli::parse_from(["closed-code", "--api-key", "test-key-123"]);
        assert_eq!(cli.api_key.unwrap(), "test-key-123");
    }
}

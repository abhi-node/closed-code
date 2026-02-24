use clap::Parser;

use closed_code::cli::{Cli, Commands};
use closed_code::config::Config;
use closed_code::repl::run_oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install panic hook to restore terminal on panic
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        default_hook(info);
    }));

    // Initialize tracing (RUST_LOG env filter)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load .env file (silently ignore if missing)
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    let config = Config::from_cli(&cli)?;

    match &cli.command {
        Some(Commands::Ask { question }) => {
            run_oneshot(&config, question).await?;
        }
        Some(Commands::Resume { session_id }) => {
            closed_code::tui::run_tui(&config, session_id.as_deref()).await?;
        }
        None => {
            closed_code::tui::run_tui(&config, None).await?;
        }
    }

    Ok(())
}

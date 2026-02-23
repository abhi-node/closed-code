pub mod app;
pub mod command_picker;
pub mod events;
pub mod gauge;
pub mod header;
pub mod input;
pub mod keybindings;
pub mod layout;
pub mod status_bar;
pub mod theme;

use crate::config::Config;

/// Launch the full-screen TUI application.
///
/// Replaces `run_repl()` as the default interactive entry point.
pub async fn run_tui(config: &Config) -> anyhow::Result<()> {
    app::run(config).await
}

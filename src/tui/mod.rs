pub mod app;
pub mod approval_overlay;
pub mod chat;
pub mod command_picker;
pub mod commands;
pub mod commit_confirm;
pub mod diff_view;
pub mod events;
pub mod file_completion;
pub mod file_indexer;
pub mod file_picker;
pub mod gauge;
pub mod header;
pub mod input;
pub mod keybindings;
pub mod layout;
pub mod markdown;
pub mod message;
pub mod mode_picker;
pub mod session_picker;
pub mod status_bar;
pub mod theme;
pub mod tui_approval_handler;

use crate::config::Config;

/// Launch the full-screen TUI application.
///
/// Replaces `run_repl()` as the default interactive entry point.
/// If `session_id` is provided, the TUI will resume that session on startup.
pub async fn run_tui(config: &Config, session_id: Option<&str>) -> anyhow::Result<()> {
    app::run(config, session_id).await
}

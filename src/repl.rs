use std::io::Write;
use std::sync::Arc;

use crossterm::style::Stylize;

use crate::agent::orchestrator::{Orchestrator, OrchestratorConfig};
use crate::config::Config;
use crate::gemini::stream::StreamEvent;
use crate::gemini::GeminiClient;
use crate::sandbox::create_sandbox;
use crate::ui::approval::{ApprovalHandler, DiffOnlyApprovalHandler};
use crate::ui::theme::Theme;

fn styled_text(text: &str, color: crossterm::style::Color) -> String {
    text.with(color).to_string()
}

/// Streaming event callback for printing to stdout.
fn default_stream_handler(event: StreamEvent) {
    match event {
        StreamEvent::TextDelta(text) => {
            print!("{}", text);
            std::io::stdout().flush().ok();
        }
        StreamEvent::Done { usage, .. } => {
            println!();
            if let Some(u) = usage {
                tracing::debug!(
                    "Tokens: {} prompt + {} completion = {} total",
                    u.prompt_token_count.unwrap_or(0),
                    u.candidates_token_count.unwrap_or(0),
                    u.total_token_count.unwrap_or(0),
                );
            }
        }
        _ => {}
    }
}

/// Run a single question in non-interactive (oneshot) mode.
pub async fn run_oneshot(config: &Config, question: &str) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let sandbox = create_sandbox(config.sandbox_mode, config.working_directory.clone());
    let approval_handler: Arc<dyn ApprovalHandler> = Arc::new(DiffOnlyApprovalHandler::new());
    let mut orchestrator = Orchestrator::new(OrchestratorConfig {
        client,
        mode: config.mode,
        working_directory: config.working_directory.clone(),
        max_output_tokens: config.max_output_tokens,
        approval_handler,
        personality: config.personality,
        context_limit_tokens: config.context_limit_tokens,
        sandbox,
        protected_paths: config.protected_paths.clone(),
    });
    orchestrator.detect_git_context().await;

    let parts = crate::agent::tag_processor::process_tags(question, &config.working_directory).await?;
    
    match orchestrator
        .handle_user_input_streaming(parts, default_stream_handler)
        .await
    {
        Ok(_) => {
            println!();
        }
        Err(e) => {
            eprintln!("\n{}: {}", styled_text("Error", Theme::ERROR), e);
        }
    }

    Ok(())
}

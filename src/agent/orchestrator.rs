use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::agent::commit_agent::CommitAgent;
use crate::agent::review_agent::ReviewAgent;
use crate::agent::AgentRequest;
use crate::config::Personality;
use crate::error::{ClosedCodeError, Result};
use crate::gemini::stream::{consume_stream, StreamEvent, StreamResult};
use crate::gemini::types::{Content, GenerateContentRequest, GenerationConfig, Part};
use crate::gemini::GeminiClient;
use crate::git::GitContext;
use crate::mode::Mode;
use crate::tool::registry::{create_orchestrator_registry, ToolRegistry};
use crate::ui::approval::ApprovalHandler;
use crate::ui::spinner::Spinner;
use crate::ui::usage::SessionUsage;

const MAX_ORCHESTRATOR_ITERATIONS: usize = 30;
const MAX_RATE_LIMIT_RETRIES: usize = 5;

/// The main orchestrator that owns the Gemini client, tool registry,
/// conversation history, and mode-specific system prompt.
///
/// The REPL creates one Orchestrator and delegates all user input through it.
pub struct Orchestrator {
    client: Arc<GeminiClient>,
    mode: Mode,
    working_directory: PathBuf,
    history: Vec<Content>,
    registry: ToolRegistry,
    system_prompt: String,
    max_output_tokens: u32,
    approval_handler: Arc<dyn ApprovalHandler>,
    current_plan: Option<String>,
    cancelled: Arc<AtomicBool>,
    // Phase 5
    personality: Personality,
    context_window_turns: usize,
    session_usage: SessionUsage,
    model_name: String,
    // Phase 6
    git_context: Option<GitContext>,
}

impl Orchestrator {
    pub fn new(
        client: Arc<GeminiClient>,
        mode: Mode,
        working_directory: PathBuf,
        max_output_tokens: u32,
        approval_handler: Arc<dyn ApprovalHandler>,
        personality: Personality,
        context_window_turns: usize,
    ) -> Self {
        let registry = create_orchestrator_registry(
            working_directory.clone(),
            &mode,
            client.clone(),
            Some(approval_handler.clone()),
        );
        let system_prompt =
            Self::build_system_prompt(&mode, &working_directory, personality, None);
        let model_name = client.model().to_string();

        Self {
            client,
            mode,
            working_directory,
            history: Vec::new(),
            registry,
            system_prompt,
            max_output_tokens,
            approval_handler,
            current_plan: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            personality,
            context_window_turns,
            session_usage: SessionUsage::new(),
            model_name,
            git_context: None,
        }
    }

    /// Handle user input with streaming callbacks for real-time display.
    ///
    /// Adds the user message to history, streams the Gemini response,
    /// executes any function calls, and returns the final assistant text.
    pub async fn handle_user_input_streaming(
        &mut self,
        input: &str,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<String> {
        self.history.push(Content::user(input));
        self.prune_history();

        let request = self.build_request();

        // Stream with rate limit retry
        let mut rate_limit_retries = 0;
        let stream_result = loop {
            let spinner = Spinner::new("Thinking...");
            let es = self.client.stream_generate_content(&request);
            let mut spinner_cleared = false;

            match consume_stream(es, |event| {
                if !spinner_cleared {
                    spinner.finish();
                    spinner_cleared = true;
                }
                on_event(event);
            })
            .await
            {
                Ok(result) => {
                    if !spinner_cleared {
                        spinner.finish();
                    }
                    break result;
                }
                Err(ClosedCodeError::RateLimited { retry_after_ms })
                    if rate_limit_retries < MAX_RATE_LIMIT_RETRIES =>
                {
                    spinner.finish();
                    let delay = crate::gemini::client::with_jitter(
                        Duration::from_millis(retry_after_ms),
                    );
                    display_rate_limit_countdown(delay).await;
                    rate_limit_retries += 1;
                    continue;
                }
                Err(e) => {
                    if !spinner_cleared {
                        spinner.finish();
                    }
                    return Err(e);
                }
            }
        };

        // Accumulate usage
        match &stream_result {
            StreamResult::Text { usage, .. }
            | StreamResult::FunctionCall { usage, .. } => {
                if let Some(u) = usage {
                    self.session_usage.accumulate(u);
                }
            }
        }

        match stream_result {
            StreamResult::Text { text, .. } => {
                self.history.push(Content::model(&text));
                Ok(text)
            }
            StreamResult::FunctionCall {
                text_so_far,
                response,
                ..
            } => {
                // Append model's function call content to history
                if let Some(candidate) = response.candidates.first() {
                    if let Some(content) = &candidate.content {
                        self.history.push(content.clone());
                    }
                }

                // Execute the initial function calls
                let mut response_parts = Vec::new();
                for part in response.function_calls() {
                    if let Part::FunctionCall { name, args, .. } = part {
                        let display = format_tool_call(name, args);
                        let tool_spinner = Spinner::new(&format!("[tool] {}", display));
                        // Clear spinner before executing — tool may show interactive UI
                        tool_spinner.finish();

                        let result = match self.registry.execute(name, args.clone()).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("Tool '{}' failed: {}", name, e);
                                serde_json::json!({"error": e.to_string()})
                            }
                        };

                        println!("\u{2713} [tool] {}", display);
                        response_parts.push(Part::FunctionResponse {
                            name: name.clone(),
                            response: result,
                        });
                    }
                }

                self.history
                    .push(Content::function_responses(response_parts));

                // Continue with the tool loop
                let loop_text = self.run_tool_loop(&mut on_event).await?;
                let mut final_text = text_so_far;
                final_text.push_str(&loop_text);
                Ok(final_text)
            }
        }
    }

    /// The streaming tool-call loop.
    ///
    /// Sends requests to Gemini, executes function calls, and repeats
    /// until a text-only response or max iterations.
    async fn run_tool_loop(
        &mut self,
        on_event: &mut impl FnMut(StreamEvent),
    ) -> Result<String> {
        let mut final_text = String::new();

        for iteration in 0..MAX_ORCHESTRATOR_ITERATIONS {
            // Check cancellation before each iteration
            if self.cancelled.load(Ordering::SeqCst) {
                tracing::info!("Orchestrator cancelled by user");
                break;
            }

            tracing::debug!(
                "Orchestrator tool loop iteration {}/{}",
                iteration + 1,
                MAX_ORCHESTRATOR_ITERATIONS
            );

            let request = self.build_request();

            // Stream with rate limit retry
            let mut rate_limit_retries = 0;
            let stream_result = loop {
                let spinner = Spinner::new("Thinking...");
                let es = self.client.stream_generate_content(&request);
                let mut spinner_cleared = false;

                match consume_stream(es, |event| {
                    if !spinner_cleared {
                        spinner.finish();
                        spinner_cleared = true;
                    }
                    on_event(event);
                })
                .await
                {
                    Ok(result) => {
                        if !spinner_cleared {
                            spinner.finish();
                        }
                        break result;
                    }
                    Err(ClosedCodeError::RateLimited { retry_after_ms })
                        if rate_limit_retries < MAX_RATE_LIMIT_RETRIES =>
                    {
                        spinner.finish();
                        let delay = crate::gemini::client::with_jitter(
                            Duration::from_millis(retry_after_ms),
                        );
                        display_rate_limit_countdown(delay).await;
                        rate_limit_retries += 1;
                        continue;
                    }
                    Err(e) => {
                        if !spinner_cleared {
                            spinner.finish();
                        }
                        return Err(e);
                    }
                }
            };

            // Accumulate usage
            match &stream_result {
                StreamResult::Text { usage, .. }
                | StreamResult::FunctionCall { usage, .. } => {
                    if let Some(u) = usage {
                        self.session_usage.accumulate(u);
                    }
                }
            }

            match stream_result {
                StreamResult::Text { text, .. } => {
                    final_text.push_str(&text);
                    self.history.push(Content::model(&text));
                    break;
                }
                StreamResult::FunctionCall {
                    text_so_far,
                    response,
                    ..
                } => {
                    final_text.push_str(&text_so_far);
                    if !text_so_far.is_empty() {
                        println!();
                    }

                    // Append model's function call content to history
                    if let Some(candidate) = response.candidates.first() {
                        if let Some(content) = &candidate.content {
                            self.history.push(content.clone());
                        }
                    }

                    // Execute all function calls
                    let mut response_parts: Vec<Part> = Vec::new();
                    for part in response.function_calls() {
                        if self.cancelled.load(Ordering::SeqCst) {
                            break;
                        }
                        if let Part::FunctionCall { name, args, .. } = part {
                            let display = format_tool_call(name, args);
                            let tool_spinner =
                                Spinner::new(&format!("[tool] {}", display));
                            // Clear spinner before executing — tool may show interactive UI
                            tool_spinner.finish();

                            let result =
                                match self.registry.execute(name, args.clone()).await {
                                    Ok(v) => v,
                                    Err(e) => {
                                        tracing::warn!("Tool '{}' failed: {}", name, e);
                                        serde_json::json!({"error": e.to_string()})
                                    }
                                };

                            println!("\u{2713} [tool] {}", display);
                            response_parts.push(Part::FunctionResponse {
                                name: name.clone(),
                                response: result,
                            });
                        }
                    }

                    self.history
                        .push(Content::function_responses(response_parts));
                }
            }
        }

        if final_text.is_empty() {
            tracing::warn!(
                "Orchestrator tool loop exhausted {} iterations without final text",
                MAX_ORCHESTRATOR_ITERATIONS
            );
        }

        Ok(final_text)
    }

    /// Build a GenerateContentRequest from current state.
    fn build_request(&self) -> GenerateContentRequest {
        let tools = self.registry.to_gemini_tools(&self.mode);
        let tool_config = tools.as_ref().map(|_| ToolRegistry::tool_config());

        GenerateContentRequest {
            contents: self.history.clone(),
            system_instruction: Some(Content::system(&self.system_prompt)),
            generation_config: Some(GenerationConfig {
                temperature: Some(1.0),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(self.max_output_tokens),
            }),
            tools,
            tool_config,
        }
    }

    /// Build the mode-specific system prompt with personality prefix.
    fn build_system_prompt(
        mode: &Mode,
        working_directory: &std::path::Path,
        personality: Personality,
        git_context: Option<&GitContext>,
    ) -> String {
        let personality_prefix = match personality {
            Personality::Friendly => {
                "You are warm, encouraging, and approachable. Use casual but \
                 professional language. Celebrate progress and be supportive \
                 when users encounter issues.\n\n"
            }
            Personality::Pragmatic => {
                "You are direct, concise, and code-focused. Get straight to \
                 the point. Prioritize accuracy and efficiency in your responses.\n\n"
            }
            Personality::None => "",
        };

        let base = format!(
            "{}You are closed-code, an AI coding assistant operating in {} mode.\n\
             Working directory: {}",
            personality_prefix,
            mode,
            working_directory.display()
        );

        let mode_section = match mode {
            Mode::Explore => {
                "\n\nYou are in EXPLORE mode. You are strictly READ-ONLY.\n\
                 You CANNOT create, modify, or delete any files.\n\
                 \n\
                 Your role is to help the user understand the codebase:\n\
                 - Read and analyze files using read_file\n\
                 - Search for patterns with search_files and grep\n\
                 - List directory contents with list_directory\n\
                 - Run read-only shell commands (git log, cargo check, etc.)\n\
                 - Use spawn_explorer for deep codebase research\n\
                 \n\
                 Explain code architecture, patterns, data flow, and answer questions.\n\
                 NEVER suggest creating or modifying files in this mode."
            }
            Mode::Plan => {
                "\n\nYou are in PLAN mode. You create implementation plans for review.\n\
                 You CANNOT modify files. Your job is to:\n\
                 1. Understand the user's requirements\n\
                 2. Research the codebase using filesystem tools and sub-agents\n\
                 3. Produce a clear, structured implementation plan with:\n\
                    - Step-by-step implementation order\n\
                    - Files to create or modify (with specific changes)\n\
                    - Code patterns to follow from the existing codebase\n\
                    - Potential risks or trade-offs\n\
                 \n\
                 Available tools:\n\
                 - spawn_explorer: Deep codebase research\n\
                 - spawn_planner: Create detailed implementation plans\n\
                 - spawn_web_search: Research topics online\n\
                 - All filesystem read tools\n\
                 \n\
                 The user will either:\n\
                 - Give feedback to refine the plan (continue the conversation)\n\
                 - Accept the plan with /accept (transitions to Execute mode)"
            }
            Mode::Execute => {
                "\n\nYou are in EXECUTE mode. You can create and edit files.\n\
                 \n\
                 Available tools:\n\
                 - write_file: Create new files or overwrite existing ones\n\
                 - edit_file: Make targeted changes using search/replace\n\
                 - spawn_explorer: Research code before making changes\n\
                 - All filesystem read tools (read_file, list_directory, search_files, grep)\n\
                 - shell: Run allowlisted commands only (ls, cat, grep, git, cargo, etc.)\n\
                 \n\
                 IMPORTANT workflow:\n\
                 1. Always read the file first (read_file) before editing it\n\
                 2. Use edit_file for targeted changes (preferred over write_file for existing files)\n\
                 3. Use write_file for new files or complete rewrites\n\
                 4. File writes are auto-approved\n\
                 5. If something goes wrong, use /explore to investigate\n\
                 \n\
                 Make changes methodically: one file at a time, with clear purpose."
            }
            Mode::Auto => {
                "\n\nYou are in AUTO mode. You have FULL autonomy.\n\
                 \n\
                 Available tools:\n\
                 - write_file: Create new files or overwrite existing ones\n\
                 - edit_file: Make targeted changes using search/replace\n\
                 - shell: Execute ANY shell command (no allowlist restrictions)\n\
                 - spawn_explorer: Research code before making changes\n\
                 - All filesystem read tools (read_file, list_directory, search_files, grep)\n\
                 \n\
                 IMPORTANT: File writes are auto-approved and shell commands are unrestricted.\n\
                 1. Always read the file first (read_file) before editing it\n\
                 2. Use edit_file for targeted changes (preferred over write_file for existing files)\n\
                 3. Use write_file for new files or complete rewrites\n\
                 4. Double-check destructive shell commands before executing\n\
                 5. Never run commands that could damage the system or delete important data\n\
                 \n\
                 Make changes methodically: one file at a time, with clear purpose."
            }
        };

        let git_section = match git_context {
            Some(ctx) => {
                let section = ctx.system_prompt_section();
                if section.is_empty() {
                    String::new()
                } else {
                    format!("\n\n{}", section)
                }
            }
            None => String::new(),
        };

        format!("{}{}{}", base, mode_section, git_section)
    }

    /// Switch to a different mode at runtime.
    /// Rebuilds the tool registry and system prompt. Preserves conversation history.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.registry = create_orchestrator_registry(
            self.working_directory.clone(),
            &self.mode,
            self.client.clone(),
            Some(self.approval_handler.clone()),
        );
        self.system_prompt = Self::build_system_prompt(
            &self.mode,
            &self.working_directory,
            self.personality,
            self.git_context.as_ref(),
        );
    }

    /// Prune conversation history when it exceeds context_window_turns.
    /// Drops the oldest half, ensuring the first entry has role "user".
    pub fn prune_history(&mut self) {
        // Warn at 80% threshold
        let threshold = (self.context_window_turns as f64 * 0.8) as usize;
        if self.history.len() == threshold {
            eprintln!(
                "Warning: Approaching context limit ({}/{} turns). Consider /clear or conversation will be pruned.",
                self.history.len(),
                self.context_window_turns,
            );
        }

        if self.history.len() <= self.context_window_turns {
            return;
        }

        let keep = self.context_window_turns / 2;
        let pruned_count = self.history.len() - keep;
        self.history = self.history.split_off(self.history.len() - keep);

        // Ensure the first message is from the user
        let first_is_user = self
            .history
            .first()
            .and_then(|c| c.role.as_deref())
            .map(|r| r == "user")
            .unwrap_or(false);

        if !first_is_user {
            self.history.insert(
                0,
                Content::user("[Earlier conversation context was pruned]"),
            );
        }

        eprintln!(
            "Context pruned: removed {} oldest turns ({} remaining)",
            pruned_count,
            self.history.len()
        );
    }

    /// Clear the conversation history.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Current mode.
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    /// Number of conversation turns.
    pub fn turn_count(&self) -> usize {
        self.history.len()
    }

    /// Reference to the system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Store the current plan text.
    /// Called by the REPL after each Plan mode response.
    pub fn set_current_plan(&mut self, plan: String) {
        self.current_plan = Some(plan);
    }

    /// Get the current plan, if any.
    pub fn current_plan(&self) -> Option<&str> {
        self.current_plan.as_deref()
    }

    /// Accept the current plan and switch to Execute mode.
    ///
    /// Injects the accepted plan into conversation history as context,
    /// then switches mode to Execute (which registers write tools).
    /// Returns the plan text if one was set, or None.
    pub fn accept_plan(&mut self) -> Option<String> {
        if let Some(plan) = self.current_plan.take() {
            self.history.push(Content::user(&format!(
                "[ACCEPTED PLAN — Execute this plan step by step]\n\n{}",
                plan
            )));
            self.set_mode(Mode::Execute);
            Some(plan)
        } else {
            None
        }
    }

    /// Get a clone of the cancellation flag for use by signal handlers.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    /// Reset the cancellation flag. Call before each model invocation.
    pub fn reset_cancel(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }

    /// Whether the model was cancelled by the user.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Record an interruption in conversation history.
    /// Adds a model message so the history stays consistent.
    pub fn record_interruption(&mut self) {
        self.history
            .push(Content::model("[Response interrupted by user]"));
    }

    // ── Phase 5 getters/setters ──

    /// Get current personality.
    pub fn personality(&self) -> Personality {
        self.personality
    }

    /// Set personality and rebuild system prompt.
    pub fn set_personality(&mut self, personality: Personality) {
        self.personality = personality;
        self.system_prompt = Self::build_system_prompt(
            &self.mode,
            &self.working_directory,
            self.personality,
            self.git_context.as_ref(),
        );
    }

    /// Get current model name.
    pub fn model(&self) -> &str {
        &self.model_name
    }

    /// Switch model. Rebuilds client and tool registry.
    pub fn set_model(&mut self, model: String) {
        self.model_name = model.clone();
        self.client = Arc::new(GeminiClient::new(
            self.client.api_key().to_string(),
            model,
        ));
        self.registry = create_orchestrator_registry(
            self.working_directory.clone(),
            &self.mode,
            self.client.clone(),
            Some(self.approval_handler.clone()),
        );
    }

    /// Get cumulative token usage.
    pub fn session_usage(&self) -> &SessionUsage {
        &self.session_usage
    }

    /// Get configured context window size.
    pub fn context_window_turns(&self) -> usize {
        self.context_window_turns
    }

    // ── Phase 6: Git Context ──

    /// Detect git context for the working directory.
    /// Call after `new()` in async contexts (REPL, oneshot).
    /// Rebuilds the system prompt with git info if in a git repo.
    pub async fn detect_git_context(&mut self) {
        let ctx = GitContext::detect(&self.working_directory).await;
        if ctx.is_git_repo {
            self.git_context = Some(ctx);
        } else {
            self.git_context = None;
        }
        self.system_prompt = Self::build_system_prompt(
            &self.mode,
            &self.working_directory,
            self.personality,
            self.git_context.as_ref(),
        );
    }

    /// Re-detect git context (e.g., after a commit).
    pub async fn refresh_git_context(&mut self) {
        self.detect_git_context().await;
    }

    /// Reference to the working directory.
    pub fn working_directory(&self) -> &std::path::Path {
        &self.working_directory
    }

    /// Get the detected default branch name, if any.
    pub fn git_default_branch(&self) -> Option<&str> {
        self.git_context
            .as_ref()
            .and_then(|ctx| ctx.default_branch.as_deref())
    }

    /// One-line git summary for display.
    pub fn git_summary(&self) -> String {
        match &self.git_context {
            Some(ctx) => ctx.summary(),
            None => "not a git repository".to_string(),
        }
    }

    // ── Phase 6: Sub-Agent Runners ──

    /// Run a commit agent to generate a commit message from a diff.
    /// Returns the commit message string. Does not modify conversation history.
    pub async fn run_commit_agent(&self, diff: &str) -> Result<String> {
        use crate::agent::Agent;

        let agent = CommitAgent::new(self.working_directory.clone());
        let request = AgentRequest::new(
            "Generate a commit message for the following code changes.".to_string(),
            self.working_directory.to_string_lossy().to_string(),
        )
        .with_context(vec![format!("```diff\n{}\n```", diff)]);

        let response = agent.run(&self.client, request).await?;
        Ok(response.summary)
    }

    /// Run a review agent to produce a structured code review from a diff.
    /// Returns the review text and injects it into conversation history
    /// so the main LLM has the review as context for follow-up questions.
    pub async fn run_review_agent(&mut self, diff: &str) -> Result<String> {
        use crate::agent::Agent;

        let agent = ReviewAgent::new(
            self.working_directory.clone(),
            self.client.clone(),
        );
        let request = AgentRequest::new(
            "Review the following code changes thoroughly.".to_string(),
            self.working_directory.to_string_lossy().to_string(),
        )
        .with_context(vec![format!("```diff\n{}\n```", diff)]);

        let response = agent.run(&self.client, request).await?;

        // Inject the review into conversation history as context for the main LLM
        self.history.push(Content::user(&format!(
            "[CODE REVIEW — Sub-agent analysis of recent changes]\n\n{}",
            response.detailed_report
        )));

        Ok(response.detailed_report)
    }
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("mode", &self.mode)
            .field("tools", &self.registry.len())
            .field("history_len", &self.history.len())
            .field("has_plan", &self.current_plan.is_some())
            .field("cancelled", &self.is_cancelled())
            .field("personality", &self.personality)
            .field("model", &self.model_name)
            .finish()
    }
}

/// Display a countdown while waiting for rate limit retry.
async fn display_rate_limit_countdown(delay: Duration) {
    use std::io::Write;
    let secs = delay.as_secs();
    for remaining in (1..=secs).rev() {
        eprint!("\rRate limited. Retrying in {}s... ", remaining);
        std::io::stderr().flush().ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    eprintln!("\rRetrying now...                    ");
}

/// Format a tool call for display: `tool_name(key: "value", key2: 123)`
pub(crate) fn format_tool_call(name: &str, args: &Value) -> String {
    let params = if let Some(obj) = args.as_object() {
        obj.iter()
            .map(|(k, v)| {
                let display_val = match v {
                    Value::String(s) => {
                        if s.len() > 60 {
                            format!("\"{}...\"", &s[..57])
                        } else {
                            format!("\"{}\"", s)
                        }
                    }
                    other => {
                        let s = other.to_string();
                        if s.len() > 60 {
                            format!("{}...", &s[..57])
                        } else {
                            s
                        }
                    }
                };
                format!("{}: {}", k, display_val)
            })
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        String::new()
    };

    format!("{}({})", name, params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::approval::AutoApproveHandler;

    fn test_client() -> Arc<GeminiClient> {
        Arc::new(GeminiClient::new("key".into(), "model".into()))
    }

    fn test_handler() -> Arc<dyn ApprovalHandler> {
        Arc::new(AutoApproveHandler::always_approve())
    }

    fn test_orchestrator() -> Orchestrator {
        Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        )
    }

    #[test]
    fn orchestrator_new_explore_mode() {
        let orch = test_orchestrator();
        // 5 filesystem/shell + spawn_explorer = 6
        assert_eq!(orch.tool_count(), 6);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert!(orch.system_prompt().contains("READ-ONLY"));
        assert!(!orch.system_prompt().contains("write_file"));
    }

    #[test]
    fn orchestrator_new_plan_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );
        // 5 filesystem/shell + spawn_explorer + spawn_planner + spawn_web_search = 8
        assert_eq!(orch.tool_count(), 8);
        assert_eq!(*orch.mode(), Mode::Plan);
        assert!(orch.system_prompt().contains("PLAN"));
        assert!(orch.system_prompt().contains("/accept"));
    }

    #[test]
    fn orchestrator_new_execute_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Execute,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );
        // 5 filesystem/shell + spawn_explorer + write_file + edit_file = 8
        assert_eq!(orch.tool_count(), 8);
        assert!(orch.system_prompt().contains("EXECUTE"));
        assert!(orch.system_prompt().contains("write_file"));
        assert!(orch.system_prompt().contains("edit_file"));
    }

    #[test]
    fn orchestrator_clear_history() {
        let mut orch = test_orchestrator();
        orch.history.push(Content::user("hello"));
        orch.history.push(Content::model("hi there"));
        assert_eq!(orch.turn_count(), 2);

        orch.clear_history();
        assert_eq!(orch.turn_count(), 0);
        assert!(orch.history.is_empty());
    }

    #[test]
    fn orchestrator_prune_history() {
        let mut orch = test_orchestrator();

        // Fill history beyond context_window_turns
        for i in 0..60 {
            if i % 2 == 0 {
                orch.history.push(Content::user(&format!("msg {}", i)));
            } else {
                orch.history.push(Content::model(&format!("reply {}", i)));
            }
        }
        assert_eq!(orch.turn_count(), 60);

        orch.prune_history();
        assert!(orch.turn_count() <= 50);

        // First entry should be role "user"
        let first_role = orch.history[0].role.as_deref();
        assert_eq!(first_role, Some("user"));
    }

    #[test]
    fn orchestrator_prune_no_op_when_small() {
        let mut orch = test_orchestrator();
        orch.history.push(Content::user("hello"));
        orch.history.push(Content::model("hi"));
        assert_eq!(orch.turn_count(), 2);

        orch.prune_history();
        assert_eq!(orch.turn_count(), 2);
    }

    #[test]
    fn prune_configurable_turns() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            20, // smaller context window
        );

        for i in 0..30 {
            if i % 2 == 0 {
                orch.history.push(Content::user(&format!("msg {}", i)));
            } else {
                orch.history.push(Content::model(&format!("reply {}", i)));
            }
        }
        assert_eq!(orch.turn_count(), 30);

        orch.prune_history();
        assert!(orch.turn_count() <= 20);
    }

    #[test]
    fn orchestrator_debug_format() {
        let orch = test_orchestrator();
        let debug = format!("{:?}", orch);
        assert!(debug.contains("Orchestrator"));
        assert!(debug.contains("Explore"));
        assert!(debug.contains("has_plan"));
        assert!(debug.contains("personality"));
        assert!(debug.contains("model"));
    }

    #[test]
    fn format_tool_call_basic() {
        let args = serde_json::json!({"path": "src/main.rs"});
        let result = format_tool_call("read_file", &args);
        assert!(result.starts_with("read_file("));
        assert!(result.contains("path:"));
        assert!(result.contains("src/main.rs"));
        assert!(result.ends_with(')'));
    }

    #[test]
    fn format_tool_call_empty_args() {
        let args = serde_json::json!({});
        let result = format_tool_call("list_directory", &args);
        assert_eq!(result, "list_directory()");
    }

    #[test]
    fn format_tool_call_truncates_long_strings() {
        let long_val = "a".repeat(100);
        let args = serde_json::json!({"content": long_val});
        let result = format_tool_call("write_file", &args);
        assert!(result.contains("..."));
    }

    #[test]
    fn orchestrator_set_mode() {
        let mut orch = test_orchestrator();
        assert_eq!(*orch.mode(), Mode::Explore);
        assert_eq!(orch.tool_count(), 6);

        // Add some history
        orch.history.push(Content::user("hello"));
        orch.history.push(Content::model("hi"));

        // Switch to Plan mode
        orch.set_mode(Mode::Plan);
        assert_eq!(*orch.mode(), Mode::Plan);
        assert_eq!(orch.tool_count(), 8);
        assert!(orch.system_prompt().contains("spawn_planner"));
        assert!(orch.system_prompt().contains("spawn_web_search"));

        // History preserved
        assert_eq!(orch.turn_count(), 2);

        // Switch to Execute mode
        orch.set_mode(Mode::Execute);
        assert_eq!(*orch.mode(), Mode::Execute);
        assert_eq!(orch.tool_count(), 8);
        assert!(orch.system_prompt().contains("write_file"));

        // Switch to Auto mode
        orch.set_mode(Mode::Auto);
        assert_eq!(*orch.mode(), Mode::Auto);
        assert_eq!(orch.tool_count(), 8);
        assert!(orch.system_prompt().contains("AUTO"));
        assert!(orch.system_prompt().contains("ANY shell command"));

        // Switch back to Explore
        orch.set_mode(Mode::Explore);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert_eq!(orch.tool_count(), 6);
        assert!(!orch.system_prompt().contains("spawn_planner"));
    }

    #[test]
    fn orchestrator_set_current_plan() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );

        assert!(orch.current_plan().is_none());
        orch.set_current_plan("Step 1: Add feature X".into());
        assert_eq!(orch.current_plan(), Some("Step 1: Add feature X"));
    }

    #[test]
    fn orchestrator_accept_plan_switches_to_execute() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );

        orch.set_current_plan("The plan content".into());
        let plan = orch.accept_plan();

        assert!(plan.is_some());
        assert_eq!(plan.unwrap(), "The plan content");
        assert_eq!(*orch.mode(), Mode::Execute);
        assert_eq!(orch.tool_count(), 8); // Now has write tools
        assert!(orch.current_plan().is_none()); // Plan consumed

        // Plan should be in history
        let last_user_msg = orch.history.last().unwrap();
        let text = last_user_msg.parts.first().unwrap();
        if let Part::Text(t) = text {
            assert!(t.contains("[ACCEPTED PLAN"));
            assert!(t.contains("The plan content"));
        } else {
            panic!("Expected text part in history");
        }
    }

    #[test]
    fn orchestrator_accept_plan_no_plan() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );

        let plan = orch.accept_plan();
        assert!(plan.is_none());
        assert_eq!(*orch.mode(), Mode::Plan); // Mode unchanged
    }

    #[test]
    fn max_orchestrator_iterations_constant() {
        assert_eq!(MAX_ORCHESTRATOR_ITERATIONS, 30);
    }

    // ── Phase 5: Personality Tests ──

    #[test]
    fn orchestrator_friendly_prompt() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::Friendly,
            50,
        );
        assert!(orch.system_prompt().contains("warm"));
        assert!(orch.system_prompt().contains("encouraging"));
    }

    #[test]
    fn orchestrator_pragmatic_prompt() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::Pragmatic,
            50,
        );
        assert!(orch.system_prompt().contains("direct"));
        assert!(orch.system_prompt().contains("concise"));
    }

    #[test]
    fn orchestrator_none_prompt() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::None,
            50,
        );
        assert!(!orch.system_prompt().contains("warm"));
        assert!(!orch.system_prompt().contains("concise, and code-focused"));
        assert!(orch.system_prompt().contains("closed-code"));
    }

    #[test]
    fn set_personality_rebuilds_prompt() {
        let mut orch = test_orchestrator();
        assert!(orch.system_prompt().contains("concise, and code-focused")); // default is Pragmatic

        orch.set_personality(Personality::Friendly);
        assert!(orch.system_prompt().contains("warm"));
        assert!(!orch.system_prompt().contains("concise, and code-focused"));
        assert_eq!(orch.personality(), Personality::Friendly);
    }

    #[test]
    fn set_mode_preserves_personality() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::Friendly,
            50,
        );
        assert!(orch.system_prompt().contains("warm"));

        orch.set_mode(Mode::Plan);
        assert!(orch.system_prompt().contains("warm"));
        assert_eq!(orch.personality(), Personality::Friendly);
    }

    // ── Phase 5: Model + Usage + Context ──

    #[test]
    fn model_getter() {
        let orch = test_orchestrator();
        assert_eq!(orch.model(), "model");
    }

    #[test]
    fn set_model_changes_name() {
        let mut orch = test_orchestrator();
        orch.set_model("gemini-2.0-flash".into());
        assert_eq!(orch.model(), "gemini-2.0-flash");
    }

    #[test]
    fn session_usage_starts_empty() {
        let orch = test_orchestrator();
        let usage = orch.session_usage();
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.api_calls, 0);
    }

    #[test]
    fn context_window_getter() {
        let orch = test_orchestrator();
        assert_eq!(orch.context_window_turns(), 50);

        let orch2 = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            100,
        );
        assert_eq!(orch2.context_window_turns(), 100);
    }

    // ── Auto Mode Tests ──

    #[test]
    fn orchestrator_new_auto_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Auto,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );
        // Same as Execute: 5 filesystem/shell + spawn_explorer + write_file + edit_file = 8
        assert_eq!(orch.tool_count(), 8);
        assert_eq!(*orch.mode(), Mode::Auto);
        assert!(orch.system_prompt().contains("AUTO"));
        assert!(orch.system_prompt().contains("ANY shell command"));
        assert!(orch.system_prompt().contains("write_file"));
        assert!(orch.system_prompt().contains("unrestricted"));
    }

    #[test]
    fn orchestrator_set_mode_auto() {
        let mut orch = test_orchestrator();
        assert_eq!(*orch.mode(), Mode::Explore);

        orch.set_mode(Mode::Auto);
        assert_eq!(*orch.mode(), Mode::Auto);
        assert_eq!(orch.tool_count(), 8);
        assert!(orch.system_prompt().contains("AUTO"));

        // Switch back
        orch.set_mode(Mode::Explore);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert_eq!(orch.tool_count(), 6);
        assert!(!orch.system_prompt().contains("AUTO"));
    }

    // ── Phase 6: Git Context Tests ──

    #[test]
    fn system_prompt_includes_git_context() {
        let git_ctx = crate::git::GitContext {
            is_git_repo: true,
            current_branch: Some("main".into()),
            default_branch: Some("main".into()),
            has_uncommitted_changes: true,
            changed_files: vec![crate::git::context::ChangedFile {
                path: "src/main.rs".into(),
                status: crate::git::context::FileStatus::Modified,
            }],
            recent_commits: vec!["abc1234 Initial commit".into()],
        };
        let prompt = Orchestrator::build_system_prompt(
            &Mode::Explore,
            std::path::Path::new("/tmp"),
            Personality::default(),
            Some(&git_ctx),
        );
        assert!(prompt.contains("Git context:"));
        assert!(prompt.contains("On branch `main`"));
        assert!(prompt.contains("src/main.rs (modified)"));
        assert!(prompt.contains("abc1234 Initial commit"));
    }

    #[test]
    fn system_prompt_no_git_context() {
        let prompt = Orchestrator::build_system_prompt(
            &Mode::Explore,
            std::path::Path::new("/tmp"),
            Personality::default(),
            None,
        );
        assert!(!prompt.contains("Git context:"));
        // Should still have the normal content
        assert!(prompt.contains("READ-ONLY"));
    }

    #[test]
    fn git_summary_without_context() {
        let orch = test_orchestrator();
        assert_eq!(orch.git_summary(), "not a git repository");
    }

    #[test]
    fn git_default_branch_without_context() {
        let orch = test_orchestrator();
        assert!(orch.git_default_branch().is_none());
    }

    #[test]
    fn working_directory_accessor() {
        let orch = test_orchestrator();
        assert_eq!(orch.working_directory(), std::path::Path::new("/tmp"));
    }

    #[tokio::test]
    async fn detect_git_context_in_non_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            dir.path().to_path_buf(),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );
        orch.detect_git_context().await;
        assert_eq!(orch.git_summary(), "not a git repository");
        assert!(orch.git_default_branch().is_none());
        assert!(!orch.system_prompt().contains("Git context:"));
    }

    #[tokio::test]
    async fn detect_git_context_in_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        // Init a git repo
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        // Create initial commit on "main" branch
        tokio::process::Command::new("git")
            .args(["checkout", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            dir.path().to_path_buf(),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );
        orch.detect_git_context().await;

        assert!(orch.git_summary().contains("main"));
        assert!(orch.system_prompt().contains("Git context:"));
    }

    #[tokio::test]
    async fn set_mode_preserves_git_context() {
        let dir = tempfile::TempDir::new().unwrap();
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["checkout", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            dir.path().to_path_buf(),
            8192,
            test_handler(),
            Personality::default(),
            50,
        );
        orch.detect_git_context().await;
        assert!(orch.system_prompt().contains("Git context:"));

        // Switch mode — git context should be preserved in the new prompt
        orch.set_mode(Mode::Execute);
        assert!(orch.system_prompt().contains("Git context:"));
        assert!(orch.system_prompt().contains("EXECUTE"));
    }

    // ── Phase 6: Sub-Agent Runner Tests ──

    #[test]
    fn commit_agent_accessible() {
        use crate::agent::commit_agent::CommitAgent;
        use crate::agent::Agent;
        let agent = CommitAgent::new(PathBuf::from("/tmp"));
        assert_eq!(agent.agent_type(), "commit");
    }

    #[test]
    fn review_agent_accessible() {
        use crate::agent::review_agent::ReviewAgent;
        use crate::agent::Agent;
        let agent = ReviewAgent::new(PathBuf::from("/tmp"), test_client());
        assert_eq!(agent.agent_type(), "reviewer");
    }
}

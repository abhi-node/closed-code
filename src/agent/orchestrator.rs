use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::agent::commit_agent::CommitAgent;
use crate::agent::review_agent::ReviewAgent;
use crate::agent::AgentRequest;
use crate::config::Personality;
use crate::error::{ClosedCodeError, Result};
use crate::gemini::stream::{consume_stream, StreamEvent, StreamResult};
use crate::gemini::types::{
    Content, CreateCachedContentRequest, GenerateContentRequest, GenerationConfig, Part,
};
use crate::gemini::GeminiClient;
use crate::git::GitContext;
use crate::mode::Mode;
use crate::sandbox::{Sandbox, SandboxMode};
use crate::session::store::SessionStore;
use crate::session::{SessionEvent, SessionId};
use crate::tool::registry::{create_orchestrator_registry, ToolRegistry};
use crate::ui::approval::ApprovalHandler;
use crate::ui::spinner::Spinner;
use crate::ui::usage::SessionUsage;
use chrono::Utc;

/// Events emitted by the orchestrator during tool execution and streaming.
/// Defined here (not in the TUI module) to avoid circular dependencies.
#[derive(Debug, Clone)]
pub enum OrchestratorEvent {
    ToolStart {
        tool_call_id: u64,
        name: String,
        args_display: String,
    },
    ToolComplete {
        tool_call_id: u64,
        name: String,
        duration: Duration,
    },
    ToolError {
        tool_call_id: u64,
        name: String,
        error: String,
    },
    AgentStart {
        agent_type: String,
        task: String,
    },
    AgentComplete {
        agent_type: String,
        duration: Duration,
    },
    AgentToolUpdate {
        agent_type: String,
        tool_name: String,
        args_display: String,
    },
}

const MAX_RATE_LIMIT_RETRIES: usize = 5;

/// Configuration for constructing an [`Orchestrator`].
pub struct OrchestratorConfig {
    pub client: Arc<GeminiClient>,
    pub mode: Mode,
    pub working_directory: PathBuf,
    pub max_output_tokens: u32,
    pub approval_handler: Arc<dyn ApprovalHandler>,
    pub personality: Personality,
    pub context_limit_tokens: u32,
    pub sandbox: Arc<dyn Sandbox>,
    pub protected_paths: Vec<String>,
}

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
    current_plan: Arc<std::sync::RwLock<Option<String>>>,
    cancelled: Arc<AtomicBool>,
    // Phase 5
    personality: Personality,
    context_limit_tokens: u32,
    last_prompt_tokens: u32,
    session_usage: SessionUsage,
    model_name: String,
    // Phase 6
    git_context: Option<GitContext>,
    // Phase 7
    sandbox: Arc<dyn Sandbox>,
    protected_paths: Vec<String>,
    // Phase 8a
    session_id: Option<SessionId>,
    session_store: Option<SessionStore>,
    // Phase 9c: TUI integration
    suppress_display: bool,
    event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
    // Phase 10: Parallel tool execution
    next_tool_call_id: AtomicU64,
    // Phase 10: Context caching
    cached_content_name: Option<String>,
    cache_fingerprint: Option<(Mode, String)>,
    cache_expire_time: Option<std::time::Instant>,
    // Sub-agent context caching
    subagent_cache_manager: Arc<crate::agent::cache::SubAgentCacheManager>,
    // Plan state tracking
    plan_accepted: bool,
}

impl Orchestrator {
    pub fn new(config: OrchestratorConfig) -> Self {
        let subagent_cache_manager =
            Arc::new(crate::agent::cache::SubAgentCacheManager::new());
        let current_plan: Arc<std::sync::RwLock<Option<String>>> =
            Arc::new(std::sync::RwLock::new(None));
        let registry = create_orchestrator_registry(
            config.working_directory.clone(),
            &config.mode,
            config.client.clone(),
            Some(config.approval_handler.clone()),
            config.sandbox.clone(),
            config.protected_paths.clone(),
            None, // event_tx set later via set_event_sender()
            subagent_cache_manager.clone(),
            current_plan.clone(),
        );
        let system_prompt = Self::build_system_prompt(
            &config.mode,
            &config.working_directory,
            config.personality,
            None,
            &*config.sandbox,
        );
        let model_name = config.client.model().to_string();

        Self {
            client: config.client,
            mode: config.mode,
            working_directory: config.working_directory,
            history: Vec::new(),
            registry,
            system_prompt,
            max_output_tokens: config.max_output_tokens,
            approval_handler: config.approval_handler,
            current_plan,
            cancelled: Arc::new(AtomicBool::new(false)),
            personality: config.personality,
            context_limit_tokens: config.context_limit_tokens,
            last_prompt_tokens: 0,
            session_usage: SessionUsage::new(),
            model_name,
            git_context: None,
            sandbox: config.sandbox,
            protected_paths: config.protected_paths,
            session_id: None,
            session_store: None,
            suppress_display: false,
            event_tx: None,
            next_tool_call_id: AtomicU64::new(0),
            cached_content_name: None,
            cache_fingerprint: None,
            cache_expire_time: None,
            subagent_cache_manager,
            plan_accepted: false,
        }
    }

    /// Handle user input with streaming callbacks for real-time display.
    ///
    /// Adds the user message to history, streams the Gemini response,
    /// executes any function calls, and returns the final assistant text.
    pub async fn handle_user_input_streaming(
        &mut self,
        parts: Vec<Part>,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<String> {
        let first_text = parts.iter().find_map(|p| {
            if let Part::Text(t) = p { Some(t.clone()) } else { None }
        }).unwrap_or_default();

        self.history.push(Content {
            role: Some("user".into()),
            parts: parts.clone(),
        });

        self.emit_event(SessionEvent::UserMessage {
            content: first_text,
            timestamp: Utc::now(),
        });

        for part in &parts {
            if let Part::InlineData { mime_type, .. } = part {
                self.emit_event(SessionEvent::ImageAttached {
                    mime_type: mime_type.clone(),
                    size_bytes: 0, // Inlined images don't strictly have a path size here, but required by struct
                    timestamp: Utc::now(),
                });
            }
        }

        if self.should_auto_compact() {
            tracing::info!(
                "Context at {}/{} tokens (95%), auto-compacting...",
                self.last_prompt_tokens, self.context_limit_tokens
            );
            on_event(StreamEvent::TextDelta(
                "\n[System: Auto-compacting conversation history...]\n".to_string(),
            ));
            let _ = self.compact_history(None).await;
        }

        self.ensure_cache().await;

        let request = self.build_request();

        // Stream with rate limit retry
        let mut rate_limit_retries = 0;
        let stream_result = loop {
            let spinner = if !self.suppress_display {
                Some(Spinner::new("Thinking..."))
            } else {
                None
            };
            let es = self.client.stream_generate_content(&request)?;
            let mut spinner_cleared = false;

            match consume_stream(es, |event| {
                if !spinner_cleared {
                    if let Some(s) = &spinner {
                        s.finish();
                    }
                    spinner_cleared = true;
                }
                on_event(event);
            })
            .await
            {
                Ok(result) => {
                    if !spinner_cleared {
                        if let Some(s) = &spinner {
                            s.finish();
                        }
                    }
                    break result;
                }
                Err(ClosedCodeError::RateLimited { retry_after_ms })
                    if rate_limit_retries < MAX_RATE_LIMIT_RETRIES =>
                {
                    if let Some(s) = &spinner {
                        s.finish();
                    }
                    let delay =
                        crate::gemini::client::with_jitter(Duration::from_millis(retry_after_ms));
                    if !self.suppress_display {
                        display_rate_limit_countdown(delay).await;
                    } else {
                        tokio::time::sleep(delay).await;
                    }
                    rate_limit_retries += 1;
                    continue;
                }
                Err(e) => {
                    if !spinner_cleared {
                        if let Some(s) = &spinner {
                            s.finish();
                        }
                    }
                    // Cache may have expired — invalidate and retry once
                    if self.cached_content_name.is_some() && is_cache_error(&e) {
                        tracing::warn!(
                            "Context cache error ({}), retrying without cache",
                            e
                        );
                        self.invalidate_cache();
                        break consume_stream(
                            self.client
                                .stream_generate_content(&self.build_request())?,
                            |event| {
                                on_event(event);
                            },
                        )
                        .await?;
                    }
                    return Err(e);
                }
            }
        };

        // Accumulate usage and track prompt tokens for context management
        match &stream_result {
            StreamResult::Text { usage, .. } | StreamResult::FunctionCall { usage, .. } => {
                if let Some(u) = usage {
                    self.session_usage.accumulate(u);
                    if let Some(pt) = u.prompt_token_count {
                        self.last_prompt_tokens = pt;
                    }
                }
            }
        }

        match stream_result {
            StreamResult::Text { text, .. } => {
                self.history.push(Content::model(&text));
                self.emit_event(SessionEvent::AssistantMessage {
                    content: text.clone(),
                    timestamp: Utc::now(),
                });
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
                        let result = self.execute_and_display_tool(name, args).await;
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
    /// until a text-only response or context is exhausted.
    async fn run_tool_loop(&mut self, on_event: &mut impl FnMut(StreamEvent)) -> Result<String> {
        let mut final_text = String::new();
        loop {
            // Check cancellation before each iteration
            if self.cancelled.load(Ordering::SeqCst) {
                tracing::info!("Orchestrator cancelled by user");
                break;
            }

            // Context-aware: auto-compact at 95% of context limit
            if self.should_auto_compact() {
                tracing::info!(
                    "Context at {}/{} tokens (95%), auto-compacting...",
                    self.last_prompt_tokens, self.context_limit_tokens
                );
                on_event(StreamEvent::TextDelta(
                    "\n[System: Auto-compacting conversation history...]\n".to_string(),
                ));
                match self.compact_history(None).await {
                    Ok(summary) => {
                        tracing::info!(
                            "Auto-compacted: {}",
                            &summary[..summary.len().min(100)]
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Auto-compact failed ({}), stopping tool loop", e);
                        break;
                    }
                }
            }

            let request = self.build_request();

            // Stream with rate limit retry
            let mut rate_limit_retries = 0;
            let stream_result = loop {
                let spinner = if !self.suppress_display {
                    Some(Spinner::new("Thinking..."))
                } else {
                    None
                };
                let es = self.client.stream_generate_content(&request)?;
                let mut spinner_cleared = false;

                match consume_stream(es, |event| {
                    if !spinner_cleared {
                        if let Some(s) = &spinner {
                            s.finish();
                        }
                        spinner_cleared = true;
                    }
                    on_event(event);
                })
                .await
                {
                    Ok(result) => {
                        if !spinner_cleared {
                            if let Some(s) = &spinner {
                                s.finish();
                            }
                        }
                        break result;
                    }
                    Err(ClosedCodeError::RateLimited { retry_after_ms })
                        if rate_limit_retries < MAX_RATE_LIMIT_RETRIES =>
                    {
                        if let Some(s) = &spinner {
                            s.finish();
                        }
                        let delay = crate::gemini::client::with_jitter(Duration::from_millis(
                            retry_after_ms,
                        ));
                        if !self.suppress_display {
                            display_rate_limit_countdown(delay).await;
                        } else {
                            tokio::time::sleep(delay).await;
                        }
                        rate_limit_retries += 1;
                        continue;
                    }
                    Err(e) => {
                        if !spinner_cleared {
                            if let Some(s) = &spinner {
                                s.finish();
                            }
                        }
                        // If the error might be due to an expired/invalid context cache,
                        // invalidate the cache and retry with inline system instruction.
                        if self.cached_content_name.is_some() && is_cache_error(&e) {
                            tracing::warn!(
                                "Context cache error ({}), retrying without cache",
                                e
                            );
                            self.invalidate_cache();
                            break consume_stream(
                                self.client
                                    .stream_generate_content(&self.build_request())?,
                                |event| {
                                    on_event(event);
                                },
                            )
                            .await?;
                        }
                        return Err(e);
                    }
                }
            };

            // Accumulate usage and track prompt tokens for context management
            match &stream_result {
                StreamResult::Text { usage, .. } | StreamResult::FunctionCall { usage, .. } => {
                    if let Some(u) = usage {
                        self.session_usage.accumulate(u);
                        if let Some(pt) = u.prompt_token_count {
                            self.last_prompt_tokens = pt;
                        }
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
                    if !text_so_far.is_empty() && !self.suppress_display {
                        println!();
                    }

                    // Append model's function call content to history
                    if let Some(candidate) = response.candidates.first() {
                        if let Some(content) = &candidate.content {
                            self.history.push(content.clone());
                        }
                    }

                    // Execute function calls (parallel for read-only, sequential for others)
                    let function_calls: Vec<&Part> = response.function_calls();
                    let mut response_parts: Vec<Part> =
                        Vec::with_capacity(function_calls.len());
                    let mut i = 0;

                    while i < function_calls.len() {
                        if self.cancelled.load(Ordering::SeqCst) {
                            break;
                        }

                        if let Part::FunctionCall { name, args, .. } = function_calls[i] {
                            if is_parallelizable(name) {
                                // Collect consecutive parallelizable calls
                                let start = i;
                                while i < function_calls.len() {
                                    if let Part::FunctionCall { name: n, .. } =
                                        function_calls[i]
                                    {
                                        if is_parallelizable(n) {
                                            i += 1;
                                        } else {
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                // Execute batch concurrently, preserving order
                                let futs: Vec<_> = function_calls[start..i]
                                    .iter()
                                    .filter_map(|part| {
                                        if let Part::FunctionCall { name, args, .. } = part {
                                            Some((name, args))
                                        } else {
                                            None
                                        }
                                    })
                                    .map(|(name, args)| async {
                                        let result =
                                            self.execute_and_display_tool(name, args).await;
                                        Part::FunctionResponse {
                                            name: name.clone(),
                                            response: result,
                                        }
                                    })
                                    .collect();
                                let results = futures::future::join_all(futs).await;
                                response_parts.extend(results);
                            } else {
                                // Sequential execution for write/shell/spawn tools
                                let result =
                                    self.execute_and_display_tool(name, args).await;
                                response_parts.push(Part::FunctionResponse {
                                    name: name.clone(),
                                    response: result,
                                });
                                i += 1;
                            }
                        } else {
                            i += 1;
                        }
                    }

                    self.history
                        .push(Content::function_responses(response_parts));
                }
            }
        }

        if final_text.is_empty() {
            tracing::warn!("Orchestrator tool loop ended without final text");
        }

        Ok(final_text)
    }

    /// Execute a tool call and display appropriate UI.
    ///
    /// For `spawn_*` tools (sub-agents), shows a box-drawing header/footer:
    /// ```text
    /// ┌ [agent:explorer] Find how auth works
    /// │  ✓ read_file(path: "src/auth.rs")
    /// └ [agent:explorer] done
    /// ```
    ///
    /// For regular tools, shows the standard spinner + checkmark pattern.
    async fn execute_and_display_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        // Cooperative cancellation for parallel batches
        if self.cancelled.load(Ordering::SeqCst) {
            return serde_json::json!({"error": "Cancelled by user"});
        }

        let tool_call_id = self.next_tool_call_id.fetch_add(1, Ordering::SeqCst);

        self.emit_event(SessionEvent::ToolCall {
            name: name.to_string(),
            args: args.clone(),
            timestamp: Utc::now(),
        });

        let start = std::time::Instant::now();

        let result = if let Some(agent_type) = name.strip_prefix("spawn_") {
            let task = args["task"]
                .as_str()
                .or_else(|| args["query"].as_str())
                .unwrap_or("...");
            let task_display = if task.len() > 80 {
                format!("{}...", &task[..77])
            } else {
                task.to_string()
            };

            if let Some(tx) = &self.event_tx {
                let _ = tx.send(OrchestratorEvent::AgentStart {
                    agent_type: agent_type.to_string(),
                    task: task_display.clone(),
                });
            } else if !self.suppress_display {
                println!("┌ [agent:{}] {}", agent_type, task_display);
            }

            let result = match self.registry.execute(name, args.clone()).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Tool '{}' failed: {}", name, e);
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(OrchestratorEvent::ToolError {
                            tool_call_id,
                            name: name.to_string(),
                            error: e.to_string(),
                        });
                    }
                    serde_json::json!({"error": e.to_string()})
                }
            };

            if let Some(tx) = &self.event_tx {
                let _ = tx.send(OrchestratorEvent::AgentComplete {
                    agent_type: agent_type.to_string(),
                    duration: start.elapsed(),
                });
            } else if !self.suppress_display {
                println!("└ [agent:{}] done", agent_type);
            }
            result
        } else {
            let display = format_tool_call(name, args);

            if let Some(tx) = &self.event_tx {
                let _ = tx.send(OrchestratorEvent::ToolStart {
                    tool_call_id,
                    name: name.to_string(),
                    args_display: display.clone(),
                });
            } else if !self.suppress_display {
                let tool_spinner = Spinner::new(&format!("[tool] {}", display));
                tool_spinner.finish();
            }

            let result = match self.registry.execute(name, args.clone()).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Tool '{}' failed: {}", name, e);
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(OrchestratorEvent::ToolError {
                            tool_call_id,
                            name: name.to_string(),
                            error: e.to_string(),
                        });
                    }
                    serde_json::json!({"error": e.to_string()})
                }
            };

            if let Some(tx) = &self.event_tx {
                let _ = tx.send(OrchestratorEvent::ToolComplete {
                    tool_call_id,
                    name: name.to_string(),
                    duration: start.elapsed(),
                });
            } else if !self.suppress_display {
                println!("\u{2713} [tool] {}", display);
            }
            result
        };

        self.emit_event(SessionEvent::ToolResponse {
            name: name.to_string(),
            result: result.to_string(),
            timestamp: Utc::now(),
        });

        result
    }

    /// Build a GenerateContentRequest from current state.
    /// When a context cache is active, references the cache instead of
    /// re-sending system_instruction/tools/tool_config.
    fn build_request(&self) -> GenerateContentRequest {
        if let Some(ref cache_name) = self.cached_content_name {
            // Cache holds system_instruction + tools + tool_config
            GenerateContentRequest {
                contents: self.history.clone(),
                system_instruction: None,
                generation_config: Some(GenerationConfig {
                    temperature: Some(1.0),
                    top_p: None,
                    top_k: None,
                    max_output_tokens: Some(self.max_output_tokens),
                }),
                tools: None,
                tool_config: None,
                cached_content: Some(cache_name.clone()),
            }
        } else {
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
                cached_content: None,
            }
        }
    }

    /// Ensure a context cache exists for the current mode and model.
    /// Creates a new cache on the first call or after a mode/model change.
    /// Refreshes TTL when the cache is close to expiry.
    async fn ensure_cache(&mut self) {
        let fingerprint = (self.mode, self.model_name.clone());
        let cache_ttl_secs = 1800; // 30 minutes
        let refresh_threshold = Duration::from_secs(cache_ttl_secs - 300); // refresh with 5 min left

        if self.cache_fingerprint.as_ref() == Some(&fingerprint) {
            // Cache fingerprint matches — check if TTL needs refresh
            if let (Some(ref name), Some(created_at)) =
                (&self.cached_content_name, self.cache_expire_time)
            {
                if created_at.elapsed() >= refresh_threshold {
                    tracing::debug!("Refreshing context cache TTL: {}", name);
                    if let Err(e) = self
                        .client
                        .update_cached_content_ttl(name, &format!("{}s", cache_ttl_secs))
                        .await
                    {
                        // TTL refresh failed — cache may have expired. Invalidate and recreate.
                        tracing::warn!("Cache TTL refresh failed ({}), will recreate", e);
                        self.cached_content_name = None;
                        self.cache_fingerprint = None;
                        self.cache_expire_time = None;
                        // Fall through to create a new cache below
                    } else {
                        self.cache_expire_time = Some(std::time::Instant::now());
                        return;
                    }
                } else {
                    return; // Cache is valid and TTL is fresh
                }
            } else {
                return;
            }
        }

        // Invalidate old cache
        if let Some(ref old_name) = self.cached_content_name {
            tracing::debug!("Deleting old context cache: {}", old_name);
            let _ = self.client.delete_cached_content(old_name).await;
            self.cached_content_name = None;
            self.cache_fingerprint = None;
            self.cache_expire_time = None;
        }

        // Create new cache with system_instruction + tools
        let tools = self.registry.to_gemini_tools(&self.mode);
        let tool_config = tools.as_ref().map(|_| ToolRegistry::tool_config());
        let request = CreateCachedContentRequest {
            model: format!("models/{}", self.model_name),
            system_instruction: Some(Content::system(&self.system_prompt)),
            tools,
            tool_config,
            ttl: format!("{}s", cache_ttl_secs),
        };

        match self.client.create_cached_content(&request).await {
            Ok(resp) => {
                tracing::info!(
                    "Created context cache: {} (tokens: {:?})",
                    resp.name,
                    resp.usage_metadata.as_ref().and_then(|m| m.total_token_count)
                );
                self.cached_content_name = Some(resp.name);
                self.cache_fingerprint = Some(fingerprint);
                self.cache_expire_time = Some(std::time::Instant::now());
            }
            Err(e) => {
                tracing::warn!("Failed to create context cache (falling back to inline): {}", e);
                self.cached_content_name = None;
                self.cache_fingerprint = None;
                self.cache_expire_time = None;
            }
        }
    }

    /// Invalidate the context cache (e.g., after mode or model change).
    fn invalidate_cache(&mut self) {
        self.cached_content_name = None;
        self.cache_fingerprint = None;
        self.cache_expire_time = None;
    }

    /// Build the mode-specific system prompt with personality prefix.
    fn build_system_prompt(
        mode: &Mode,
        working_directory: &std::path::Path,
        personality: Personality,
        git_context: Option<&GitContext>,
        sandbox: &dyn Sandbox,
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
                 CONTEXT CONSERVATION: When your task involves researching multiple \
                 files, tracing code paths, or understanding unfamiliar areas of the \
                 codebase, prefer spawn_explorer over reading files directly. Sub-agents \
                 run in isolated contexts and return structured reports, keeping your \
                 main context window clean. Use direct tools (read_file, grep) only for \
                 quick single-file lookups where you already know exactly what you need.\n\
                 \n\
                 Explain code architecture, patterns, data flow, and answer questions.\n\
                 NEVER suggest creating or modifying files in this mode."
            }
            Mode::Plan => {
                "\n\nYou are in PLAN mode. You create implementation plans for review.\n\
                 You CANNOT modify files. Your job is to:\n\
                 1. Understand the user's requirements\n\
                 2. Research the codebase using filesystem tools and sub-agents\n\
                 3. Produce a clear, structured implementation plan\n\
                 \n\
                 Available tools:\n\
                 - spawn_explorer: Deep codebase research (use for multi-file analysis)\n\
                 - spawn_planner: Create detailed implementation plans\n\
                 - spawn_web_search: Research topics online\n\
                 - All filesystem read tools\n\
                 \n\
                 CONTEXT CONSERVATION: Delegate research to sub-agents whenever possible. \
                 Use spawn_explorer to investigate code architecture, trace dependencies, \
                 or analyze multiple files. Use spawn_planner for generating detailed \
                 step-by-step plans from complex requirements. This keeps your main context \
                 focused on the user conversation rather than raw file contents. Reserve \
                 direct tool use (read_file, grep) for quick, targeted lookups.\n\
                 \n\
                 IMPORTANT: You MUST always end your response with a concrete implementation \
                 plan. Even if you have questions or need clarification, still present your \
                 best plan based on current understanding. The plan should be clearly \
                 formatted with:\n\
                 - Step-by-step implementation order (numbered)\n\
                 - Files to create or modify (with specific changes described)\n\
                 - Code patterns to follow from the existing codebase\n\
                 - Potential risks or trade-offs\n\
                 \n\
                 The user can then:\n\
                 - Give feedback to refine the plan (continue the conversation)\n\
                 - Accept the plan with /accept (choose Guided, Execute, or Auto mode)"
            }
            Mode::Guided => {
                "\n\nYou are in GUIDED mode. You can create and edit files, but \
                 EVERY change requires explicit user approval.\n\
                 \n\
                 Available tools:\n\
                 - write_file: Create/overwrite files (requires user approval)\n\
                 - edit_file: Targeted search/replace changes (requires user approval)\n\
                 - spawn_explorer: Research code before making changes\n\
                 - spawn_planner: Plan complex multi-step changes before executing\n\
                 - get_plan: Retrieve the current accepted plan (use to check remaining steps)\n\
                 - All filesystem read tools (read_file, list_directory, search_files, grep)\n\
                 - shell: Run allowlisted commands only (ls, cat, grep, git, cargo, etc.)\n\
                 \n\
                 CONTEXT CONSERVATION: Before making changes, use sub-agents to research \
                 the codebase. spawn_explorer for understanding code you need to modify, \
                 spawn_planner for breaking down complex tasks into steps. This avoids \
                 filling your context with raw file contents and produces better plans. \
                 Use direct tools (read_file) only for quick lookups of files you already \
                 know about.\n\
                 \n\
                 IMPORTANT workflow:\n\
                 1. If a plan was accepted, check it with get_plan before starting\n\
                 2. Research first: use sub-agents to understand the code before editing\n\
                 3. Always read the file (read_file) immediately before editing it\n\
                 4. Use edit_file for targeted changes (preferred over write_file for existing files)\n\
                 5. Use write_file for new files or complete rewrites\n\
                 6. Each change shows a diff the user must approve or reject\n\
                 7. If the user rejects a change, adjust your approach based on their feedback\n\
                 \n\
                 Make changes methodically: one file at a time, with clear purpose."
            }
            Mode::Execute => {
                "\n\nYou are in EXECUTE mode. You can create and edit files.\n\
                 \n\
                 Available tools:\n\
                 - write_file: Create new files or overwrite existing ones\n\
                 - edit_file: Make targeted changes using search/replace\n\
                 - spawn_explorer: Research code before making changes\n\
                 - spawn_planner: Plan complex multi-step changes before executing\n\
                 - get_plan: Retrieve the current accepted plan (use to check remaining steps)\n\
                 - All filesystem read tools (read_file, list_directory, search_files, grep)\n\
                 - shell: Run allowlisted commands only (ls, cat, grep, git, cargo, etc.)\n\
                 \n\
                 CONTEXT CONSERVATION: Before making changes, use sub-agents to research \
                 the codebase. spawn_explorer for understanding code you need to modify, \
                 spawn_planner for breaking down complex tasks into steps. This avoids \
                 filling your context with raw file contents and produces better plans. \
                 Use direct tools (read_file) only for quick lookups of files you already \
                 know about.\n\
                 \n\
                 IMPORTANT workflow:\n\
                 1. If a plan was accepted, check it with get_plan before starting\n\
                 2. Research first: use sub-agents to understand the code before editing\n\
                 3. Always read the file (read_file) immediately before editing it\n\
                 4. Use edit_file for targeted changes (preferred over write_file for existing files)\n\
                 5. Use write_file for new files or complete rewrites\n\
                 6. File writes are auto-approved\n\
                 7. If something goes wrong, use /explore to investigate\n\
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
                 - spawn_planner: Plan complex multi-step changes before executing\n\
                 - get_plan: Retrieve the current accepted plan (use to check remaining steps)\n\
                 - All filesystem read tools (read_file, list_directory, search_files, grep)\n\
                 \n\
                 CONTEXT CONSERVATION: Before making changes, use sub-agents to research \
                 the codebase. spawn_explorer for understanding code you need to modify, \
                 spawn_planner for breaking down complex tasks into steps. This avoids \
                 filling your context with raw file contents and produces better plans. \
                 Use direct tools (read_file) only for quick lookups of files you already \
                 know about.\n\
                 \n\
                 IMPORTANT: File writes are auto-approved and shell commands are unrestricted.\n\
                 1. If a plan was accepted, check it with get_plan before starting\n\
                 2. Research first: use sub-agents to understand the code before editing\n\
                 3. Always read the file (read_file) immediately before editing it\n\
                 4. Use edit_file for targeted changes (preferred over write_file for existing files)\n\
                 5. Use write_file for new files or complete rewrites\n\
                 6. Double-check destructive shell commands before executing\n\
                 7. Never run commands that could damage the system or delete important data\n\
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

        let sandbox_section = if sandbox.mode() != SandboxMode::FullAccess {
            let (read_policy, write_policy, network_policy) = match sandbox.mode() {
                SandboxMode::WorkspaceOnly => (
                    "workspace + system paths only",
                    "workspace directory only",
                    "blocked",
                ),
                SandboxMode::WorkspaceWrite => {
                    ("everywhere", "workspace directory only", "allowed")
                }
                SandboxMode::FullAccess => unreachable!(),
            };
            format!(
                "\n\nSandbox: {} ({})\n  \
                 - File reads: {}. File writes: {}.\n  \
                 - Network: {}.\n  \
                 - Protected paths: .git/, .closed-code/, .env, *.pem, *.key",
                sandbox.mode(),
                sandbox.backend(),
                read_policy,
                write_policy,
                network_policy,
            )
        } else {
            String::new()
        };

        format!("{}{}{}{}", base, mode_section, git_section, sandbox_section)
    }

    /// Switch to a different mode at runtime.
    /// Rebuilds the tool registry and system prompt. Preserves conversation history.
    pub fn set_mode(&mut self, mode: Mode) {
        let old_mode = self.mode;
        self.mode = mode;
        self.invalidate_cache();
        // Sub-agent caches may have different tool sets per mode — drain them.
        // Server-side caches expire via TTL; we just forget the names.
        let _old = self.subagent_cache_manager.drain_all();
        self.emit_event(SessionEvent::ModeChange {
            from: old_mode.to_string(),
            to: mode.to_string(),
            timestamp: Utc::now(),
        });
        self.registry = create_orchestrator_registry(
            self.working_directory.clone(),
            &self.mode,
            self.client.clone(),
            Some(self.approval_handler.clone()),
            self.sandbox.clone(),
            self.protected_paths.clone(),
            self.event_tx.clone(),
            self.subagent_cache_manager.clone(),
            self.current_plan.clone(),
        );
        self.system_prompt = Self::build_system_prompt(
            &self.mode,
            &self.working_directory,
            self.personality,
            self.git_context.as_ref(),
            &*self.sandbox,
        );
    }

    /// Switch mode with an optional new approval handler.
    /// If a handler is provided, it replaces the current one before rebuilding
    /// the registry (so the new tools use the new handler).
    pub fn set_mode_with_handler(&mut self, mode: Mode, handler: Option<Arc<dyn ApprovalHandler>>) {
        if let Some(h) = handler {
            self.approval_handler = h;
        }
        self.set_mode(mode);
    }

    /// Check if context is at 95% capacity, triggering auto-compaction.
    fn should_auto_compact(&self) -> bool {
        self.last_prompt_tokens > 0
            && self.context_limit_tokens > 0
            && self.last_prompt_tokens >= (self.context_limit_tokens as f64 * 0.95) as u32
    }

    /// Clear the conversation history.
    /// If a session is active, emits SessionEnd and starts a new session.
    pub fn clear_history(&mut self) {
        self.emit_event(SessionEvent::SessionEnd {
            timestamp: Utc::now(),
        });
        self.history.clear();
        // Start new session if store configured
        if self.session_store.is_some() {
            let new_id = SessionId::new();
            let event = SessionEvent::SessionStart {
                session_id: new_id.clone(),
                model: self.model_name.clone(),
                mode: self.mode.to_string(),
                working_directory: self.working_directory.display().to_string(),
                timestamp: Utc::now(),
            };
            if let Some(store) = &self.session_store {
                if let Err(e) = store.save_event(&new_id, &event) {
                    tracing::warn!("Failed to save new SessionStart: {}", e);
                }
            }
            self.session_id = Some(new_id);
        }
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
        if let Ok(mut guard) = self.current_plan.write() {
            *guard = Some(plan);
        }
        self.plan_accepted = false;
    }

    /// Get the current plan text, if any.
    pub fn current_plan_text(&self) -> Option<String> {
        self.current_plan.read().ok().and_then(|g| g.clone())
    }

    /// Get a shared reference to the plan state (for tools).
    pub fn plan_handle(&self) -> Arc<std::sync::RwLock<Option<String>>> {
        self.current_plan.clone()
    }

    /// Accept the current plan and switch to the specified mode.
    ///
    /// Injects the accepted plan into conversation history as context,
    /// then switches to the target mode (which registers write tools).
    /// The plan remains available via `get_plan` tool.
    /// Returns the plan text if one was set, or None.
    pub fn accept_plan(&mut self, target_mode: Mode) -> Option<String> {
        let plan = self.current_plan_text();
        if let Some(ref plan_text) = plan {
            if !self.plan_accepted {
                self.history.push(Content::user(&format!(
                    "[ACCEPTED PLAN — Execute this plan step by step]\n\n{}",
                    plan_text
                )));
                self.plan_accepted = true;
            }
            self.set_mode(target_mode);
        }
        plan
    }

    /// Whether the current plan has been accepted.
    pub fn is_plan_accepted(&self) -> bool {
        self.plan_accepted
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
            &*self.sandbox,
        );
    }

    /// Get current model name.
    pub fn model(&self) -> &str {
        &self.model_name
    }

    /// Switch model. Rebuilds client and tool registry.
    pub fn set_model(&mut self, model: String) {
        self.model_name = model.clone();
        self.invalidate_cache();
        // Drain sub-agent caches — model changed, old caches are for the wrong model.
        let _old = self.subagent_cache_manager.drain_all();
        self.client = Arc::new(GeminiClient::new(self.client.api_key().to_string(), model));
        self.registry = create_orchestrator_registry(
            self.working_directory.clone(),
            &self.mode,
            self.client.clone(),
            Some(self.approval_handler.clone()),
            self.sandbox.clone(),
            self.protected_paths.clone(),
            self.event_tx.clone(),
            self.subagent_cache_manager.clone(),
            self.current_plan.clone(),
        );
    }

    /// Get cumulative token usage.
    pub fn session_usage(&self) -> &SessionUsage {
        &self.session_usage
    }

    /// Get last prompt token count from the most recent API call.
    pub fn last_prompt_tokens(&self) -> u32 {
        self.last_prompt_tokens
    }

    /// Get the model's context limit in tokens.
    pub fn context_limit_tokens(&self) -> u32 {
        self.context_limit_tokens
    }

    // ── Phase 8a: Session Management ──

    /// Set session ID and store for this orchestrator.
    pub fn set_session(&mut self, id: SessionId, store: SessionStore) {
        self.session_id = Some(id);
        self.session_store = Some(store);
    }

    /// Get current session ID.
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// Get reference to session store.
    pub fn session_store(&self) -> Option<&SessionStore> {
        self.session_store.as_ref()
    }

    /// Fire-and-forget session event emission.
    pub fn emit_event(&self, event: SessionEvent) {
        if let (Some(session_id), Some(store)) = (&self.session_id, &self.session_store) {
            if let Err(e) = store.save_event(session_id, &event) {
                tracing::warn!("Failed to save session event: {}", e);
            }
        }
    }

    /// Replace history (for resume/compact).
    pub fn set_history(&mut self, history: Vec<Content>) {
        self.history = history;
    }

    /// Get history reference.
    pub fn history(&self) -> &[Content] {
        &self.history
    }

    /// Start a new session with auto-generated ID.
    pub fn start_session(&mut self, store: SessionStore) {
        let session_id = SessionId::new();
        let event = SessionEvent::SessionStart {
            session_id: session_id.clone(),
            model: self.model_name.clone(),
            mode: self.mode.to_string(),
            working_directory: self.working_directory.display().to_string(),
            timestamp: Utc::now(),
        };
        if let Err(e) = store.save_event(&session_id, &event) {
            tracing::warn!("Failed to save SessionStart: {}", e);
        }
        self.session_id = Some(session_id);
        self.session_store = Some(store);
    }

    /// Fork current session into new ID.
    pub fn fork_session(&mut self) -> Result<Option<SessionId>> {
        let (session_id, store) = match (&self.session_id, &self.session_store) {
            (Some(id), Some(store)) => (id.clone(), store.clone()),
            _ => return Ok(None),
        };
        let new_id = SessionId::new();
        store.fork_session(&session_id, &new_id)?;
        // Mark fork point
        store.save_event(
            &new_id,
            &SessionEvent::SessionStart {
                session_id: new_id.clone(),
                model: self.model_name.clone(),
                mode: self.mode.to_string(),
                working_directory: self.working_directory.display().to_string(),
                timestamp: Utc::now(),
            },
        )?;
        self.session_id = Some(new_id.clone());
        Ok(Some(new_id))
    }

    /// Compact conversation via LLM summarization.
    /// `user_prompt` is an optional instruction for how to summarize.
    /// Always keeps the last 5 recent turns.
    pub async fn compact_history(&mut self, user_prompt: Option<&str>) -> Result<String> {
        const KEEP_RECENT: usize = 5;

        if self.history.len() <= KEEP_RECENT + 1 {
            return Err(ClosedCodeError::SessionError(
                "History too short to compact (need more than 6 turns)".into(),
            ));
        }

        let turns_before = self.history.len();

        // Build text representation of history for summarization
        let mut history_text = String::new();
        for content in &self.history {
            let role = content.role.as_deref().unwrap_or("system");
            for part in &content.parts {
                match part {
                    Part::Text(t) => {
                        history_text.push_str(&format!("[{}]: {}\n\n", role, t));
                    }
                    Part::FunctionCall { name, args, .. } => {
                        history_text
                            .push_str(&format!("[{}]: Called tool {}({})\n\n", role, name, args));
                    }
                    Part::FunctionResponse { name, response, .. } => {
                        let resp_str = response.to_string();
                        let truncated = if resp_str.chars().count() > 200 {
                            format!("{}...", &resp_str[..truncate_byte_index(&resp_str, 197)])
                        } else {
                            resp_str
                        };
                        history_text.push_str(&format!(
                            "[{}]: Tool {} returned: {}\n\n",
                            role, name, truncated
                        ));
                    }
                    Part::InlineData { mime_type, .. } => {
                        history_text.push_str(&format!("[{}]: [Image: {}]\n\n", role, mime_type));
                    }
                }
            }
        }

        let summarization_instruction = match user_prompt {
            Some(prompt) => format!(
                "Focus on: {}. Summarize this conversation in 500 words or fewer. \
                 Preserve key decisions, code changes, file paths mentioned, and any important context.",
                prompt
            ),
            None => "Summarize this conversation in 500 words or fewer. \
                     Preserve key decisions, code changes, file paths mentioned, and any important context."
                .to_string(),
        };

        let request = GenerateContentRequest {
            contents: vec![Content::user(&format!(
                "{}\n\n---\n\n{}",
                summarization_instruction, history_text
            ))],
            system_instruction: Some(Content::system(
                "You are a conversation summarizer. Produce a concise summary that preserves \
                 the most important information: decisions made, files modified, code patterns \
                 discussed, and any unresolved issues. Output ONLY the summary text.",
            )),
            generation_config: Some(GenerationConfig {
                temperature: Some(0.3),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(2048),
            }),
            tools: None,
            tool_config: None,
            cached_content: None,
        };

        let response = self.client.generate_content(&request).await?;
        let summary = response
            .text()
            .ok_or_else(|| {
                ClosedCodeError::SessionError("Compact: empty summary from model".into())
            })?
            .to_string();

        // Keep last N turns
        let recent_start = self.history.len().saturating_sub(KEEP_RECENT);
        let recent_turns: Vec<Content> = self.history[recent_start..].to_vec();

        // Replace history: summary + recent turns
        self.history = Vec::new();
        self.history.push(Content::user(&format!(
            "[Previous conversation summary]: {}",
            summary
        )));
        self.history.extend(recent_turns);

        let turns_after = self.history.len();

        self.emit_event(SessionEvent::Compact {
            summary: summary.clone(),
            turns_before,
            turns_after,
            timestamp: Utc::now(),
        });

        Ok(summary)
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
            &*self.sandbox,
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

    // ── Phase 7: Sandbox accessors ──

    /// Get the current sandbox mode.
    pub fn sandbox_mode(&self) -> SandboxMode {
        self.sandbox.mode()
    }

    /// One-line sandbox summary for display.
    pub fn sandbox_summary(&self) -> String {
        format!("{} ({})", self.sandbox.mode(), self.sandbox.backend())
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

    // ── Phase 9c: TUI Integration ──

    /// Suppress terminal display (Spinner, println) for TUI mode.
    pub fn set_suppress_display(&mut self, suppress: bool) {
        self.suppress_display = suppress;
    }

    /// Set the event sender for tool/agent notifications.
    /// Rebuilds the registry so spawn tools get the event channel.
    pub fn set_event_sender(&mut self, tx: mpsc::UnboundedSender<OrchestratorEvent>) {
        self.event_tx = Some(tx);
        // Rebuild registry so spawn tools receive the event channel
        self.registry = create_orchestrator_registry(
            self.working_directory.clone(),
            &self.mode,
            self.client.clone(),
            Some(self.approval_handler.clone()),
            self.sandbox.clone(),
            self.protected_paths.clone(),
            self.event_tx.clone(),
            self.subagent_cache_manager.clone(),
            self.current_plan.clone(),
        );
    }

    /// Number of tools available for a given mode.
    pub fn tool_count_for_mode(&self, mode: &Mode) -> usize {
        let registry = create_orchestrator_registry(
            self.working_directory.clone(),
            mode,
            self.client.clone(),
            Some(self.approval_handler.clone()),
            self.sandbox.clone(),
            self.protected_paths.clone(),
            None,
            self.subagent_cache_manager.clone(),
            self.current_plan.clone(),
        );
        registry.len()
    }

    /// Get the configured protected paths.
    pub fn protected_paths(&self) -> &[String] {
        &self.protected_paths
    }

    /// Format the last N history entries for display.
    pub fn recent_history_display(&self, n: usize) -> String {
        let start = self.history.len().saturating_sub(n);
        let mut output = String::new();
        for (i, content) in self.history[start..].iter().enumerate() {
            let role = content.role.as_deref().unwrap_or("system");
            for part in &content.parts {
                match part {
                    Part::Text(t) => {
                        let truncated = if t.len() > 200 {
                            format!("{}...", &t[..197])
                        } else {
                            t.clone()
                        };
                        output.push_str(&format!("[{}] {}: {}\n", start + i + 1, role, truncated));
                    }
                    Part::FunctionCall { name, .. } => {
                        output.push_str(&format!(
                            "[{}] {}: tool call: {}\n",
                            start + i + 1,
                            role,
                            name
                        ));
                    }
                    Part::FunctionResponse { name, .. } => {
                        output.push_str(&format!(
                            "[{}] {}: tool result: {}\n",
                            start + i + 1,
                            role,
                            name
                        ));
                    }
                    Part::InlineData { mime_type, .. } => {
                        output.push_str(&format!(
                            "[{}] {}: [{}]\n",
                            start + i + 1,
                            role,
                            mime_type
                        ));
                    }
                }
            }
        }
        if output.is_empty() {
            "No conversation history.".to_string()
        } else {
            output
        }
    }

    /// Export the current session to a markdown file.
    pub fn export_session(&self, path: &str) -> Result<()> {
        let mut output = String::from("# Session Export\n\n");
        if let Some(id) = &self.session_id {
            output.push_str(&format!("Session: {}\n\n", id));
        }
        output.push_str(&format!(
            "Mode: {}\nModel: {}\n\n---\n\n",
            self.mode, self.model_name
        ));

        for content in &self.history {
            let role = content.role.as_deref().unwrap_or("system");
            for part in &content.parts {
                match part {
                    Part::Text(t) => {
                        output.push_str(&format!("## {}\n\n{}\n\n", role, t));
                    }
                    Part::FunctionCall { name, args, .. } => {
                        output.push_str(&format!(
                            "### Tool Call: {}\n\n```json\n{}\n```\n\n",
                            name,
                            serde_json::to_string_pretty(args).unwrap_or_default()
                        ));
                    }
                    Part::FunctionResponse { name, response, .. } => {
                        let resp_str = response.to_string();
                        let truncated = if resp_str.chars().count() > 1000 {
                            format!("{}...", &resp_str[..truncate_byte_index(&resp_str, 997)])
                        } else {
                            resp_str
                        };
                        output.push_str(&format!(
                            "### Tool Result: {}\n\n```\n{}\n```\n\n",
                            name, truncated
                        ));
                    }
                    Part::InlineData { mime_type, .. } => {
                        output.push_str(&format!("*[Inline data: {}]*\n\n", mime_type));
                    }
                }
            }
        }

        std::fs::write(path, output)
            .map_err(|e| ClosedCodeError::SessionError(format!("Failed to export session: {}", e)))
    }

    // ── Phase 6: Sub-Agent Runners ──

    /// Run a commit agent to generate a commit message from a diff.
    /// Returns the commit message string. Does not modify conversation history.
    pub async fn run_commit_agent(&self, diff: &str) -> Result<String> {
        use crate::agent::Agent;

        let mut agent = CommitAgent::new(self.working_directory.clone(), self.sandbox.clone())
            .with_cache_manager(self.subagent_cache_manager.clone());
        if let Some(ref tx) = self.event_tx {
            agent = agent.with_progress(crate::tool::spawn::make_agent_progress_callback(
                tx.clone(),
                "commit",
            ));
        }
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

        let mut agent = ReviewAgent::new(self.working_directory.clone(), self.sandbox.clone())
            .with_cache_manager(self.subagent_cache_manager.clone());
        if let Some(ref tx) = self.event_tx {
            agent = agent.with_progress(crate::tool::spawn::make_agent_progress_callback(
                tx.clone(),
                "review",
            ));
        }
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
            .field("has_plan", &self.current_plan_text().is_some())
            .field("cancelled", &self.is_cancelled())
            .field("personality", &self.personality)
            .field("model", &self.model_name)
            .field("session_id", &self.session_id)
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

/// Whether an error is likely caused by an expired or invalid context cache.
fn is_cache_error(err: &ClosedCodeError) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("cached")
        || msg.contains("cache")
        || msg.contains("not found")
        || msg.contains("invalid")
}

/// Whether a tool is safe to run concurrently with other tools.
/// Only pure read-only filesystem tools are parallelizable.
fn is_parallelizable(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file" | "list_directory" | "search_files" | "grep"
    )
}

/// Return the byte index of the `n`-th char boundary, or `s.len()` if fewer chars exist.
/// This is safe for slicing: `&s[..truncate_byte_index(s, n)]` will never panic.
fn truncate_byte_index(s: &str, max_chars: usize) -> usize {
    s.char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len())
}

/// Format a tool call for display: `tool_name(key: "value", key2: 123)`
pub(crate) fn format_tool_call(name: &str, args: &Value) -> String {
    let params = if let Some(obj) = args.as_object() {
        obj.iter()
            .map(|(k, v)| {
                let display_val = match v {
                    Value::String(s) => {
                        if s.chars().count() > 60 {
                            format!("\"{}...\"", &s[..truncate_byte_index(s, 57)])
                        } else {
                            format!("\"{}\"", s)
                        }
                    }
                    other => {
                        let s = other.to_string();
                        if s.chars().count() > 60 {
                            format!("{}...", &s[..truncate_byte_index(&s, 57)])
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
    use crate::sandbox::mock::MockSandbox;
    use crate::ui::approval::AutoApproveHandler;

    fn test_client() -> Arc<GeminiClient> {
        Arc::new(GeminiClient::new("key".into(), "model".into()))
    }

    fn test_handler() -> Arc<dyn ApprovalHandler> {
        Arc::new(AutoApproveHandler::always_approve())
    }

    fn mock_sandbox() -> Arc<dyn Sandbox> {
        Arc::new(MockSandbox::new(PathBuf::from("/tmp")))
    }

    fn test_config() -> OrchestratorConfig {
        OrchestratorConfig {
            client: test_client(),
            mode: Mode::Explore,
            working_directory: PathBuf::from("/tmp"),
            max_output_tokens: 8192,
            approval_handler: test_handler(),
            personality: Personality::default(),
            context_limit_tokens: 1_000_000,
            sandbox: mock_sandbox(),
            protected_paths: vec![],
        }
    }

    fn test_orchestrator() -> Orchestrator {
        Orchestrator::new(test_config())
    }

    #[test]
    fn orchestrator_new_explore_mode() {
        let orch = test_orchestrator();
        // 5 filesystem/shell + spawn_explorer + get_plan = 7
        assert_eq!(orch.tool_count(), 7);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert!(orch.system_prompt().contains("READ-ONLY"));
        assert!(!orch.system_prompt().contains("write_file"));
    }

    #[test]
    fn orchestrator_new_plan_mode() {
        let orch = Orchestrator::new(OrchestratorConfig {
            mode: Mode::Plan,
            ..test_config()
        });
        // 5 filesystem/shell + spawn_explorer + spawn_planner + spawn_web_search + get_plan = 9
        assert_eq!(orch.tool_count(), 9);
        assert_eq!(*orch.mode(), Mode::Plan);
        assert!(orch.system_prompt().contains("PLAN"));
        assert!(orch.system_prompt().contains("/accept"));
    }

    #[test]
    fn orchestrator_new_execute_mode() {
        let orch = Orchestrator::new(OrchestratorConfig {
            mode: Mode::Execute,
            ..test_config()
        });
        // 5 filesystem/shell + spawn_explorer + spawn_planner + write_file + edit_file + get_plan = 10
        assert_eq!(orch.tool_count(), 10);
        assert!(orch.system_prompt().contains("EXECUTE"));
        assert!(orch.system_prompt().contains("write_file"));
        assert!(orch.system_prompt().contains("edit_file"));
        assert!(orch.system_prompt().contains("spawn_planner"));
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
    fn should_auto_compact_below_threshold() {
        let orch = test_orchestrator();
        // No tokens tracked yet — should not compact
        assert!(!orch.should_auto_compact());
    }

    #[test]
    fn should_auto_compact_at_95_percent() {
        let mut orch = Orchestrator::new(OrchestratorConfig {
            context_limit_tokens: 1_000_000,
            ..test_config()
        });
        // Below 95% — no compact
        orch.last_prompt_tokens = 940_000;
        assert!(!orch.should_auto_compact());

        // At 95% — should compact
        orch.last_prompt_tokens = 950_000;
        assert!(orch.should_auto_compact());
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
    fn is_parallelizable_read_only_tools() {
        assert!(is_parallelizable("read_file"));
        assert!(is_parallelizable("list_directory"));
        assert!(is_parallelizable("search_files"));
        assert!(is_parallelizable("grep"));
    }

    #[test]
    fn is_parallelizable_rejects_write_tools() {
        assert!(!is_parallelizable("write_file"));
        assert!(!is_parallelizable("edit_file"));
        assert!(!is_parallelizable("shell"));
        assert!(!is_parallelizable("spawn_explorer"));
        assert!(!is_parallelizable("spawn_planner"));
        assert!(!is_parallelizable("spawn_web_search"));
        assert!(!is_parallelizable("create_report"));
        assert!(!is_parallelizable("unknown_tool"));
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
        assert_eq!(orch.tool_count(), 7);

        // Add some history
        orch.history.push(Content::user("hello"));
        orch.history.push(Content::model("hi"));

        // Switch to Plan mode
        orch.set_mode(Mode::Plan);
        assert_eq!(*orch.mode(), Mode::Plan);
        assert_eq!(orch.tool_count(), 9);
        assert!(orch.system_prompt().contains("spawn_planner"));
        assert!(orch.system_prompt().contains("spawn_web_search"));

        // History preserved
        assert_eq!(orch.turn_count(), 2);

        // Switch to Execute mode
        orch.set_mode(Mode::Execute);
        assert_eq!(*orch.mode(), Mode::Execute);
        assert_eq!(orch.tool_count(), 10);
        assert!(orch.system_prompt().contains("write_file"));
        assert!(orch.system_prompt().contains("spawn_planner"));

        // Switch to Auto mode
        orch.set_mode(Mode::Auto);
        assert_eq!(*orch.mode(), Mode::Auto);
        assert_eq!(orch.tool_count(), 10);
        assert!(orch.system_prompt().contains("AUTO"));
        assert!(orch.system_prompt().contains("ANY shell command"));

        // Switch back to Explore
        orch.set_mode(Mode::Explore);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert_eq!(orch.tool_count(), 7);
        assert!(!orch.system_prompt().contains("spawn_planner"));
    }

    #[test]
    fn orchestrator_set_current_plan() {
        let mut orch = Orchestrator::new(OrchestratorConfig {
            mode: Mode::Plan,
            ..test_config()
        });

        assert!(orch.current_plan_text().is_none());
        orch.set_current_plan("Step 1: Add feature X".into());
        assert_eq!(orch.current_plan_text().as_deref(), Some("Step 1: Add feature X"));
    }

    #[test]
    fn orchestrator_accept_plan_switches_to_execute() {
        let mut orch = Orchestrator::new(OrchestratorConfig {
            mode: Mode::Plan,
            ..test_config()
        });

        orch.set_current_plan("The plan content".into());
        let plan = orch.accept_plan(Mode::Execute);

        assert!(plan.is_some());
        assert_eq!(plan.unwrap(), "The plan content");
        assert_eq!(*orch.mode(), Mode::Execute);
        assert_eq!(orch.tool_count(), 10); // write + spawn_planner + get_plan tools
        // Plan persists after acceptance (available via get_plan tool)
        assert!(orch.current_plan_text().is_some());

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
        let mut orch = Orchestrator::new(OrchestratorConfig {
            mode: Mode::Plan,
            ..test_config()
        });

        let plan = orch.accept_plan(Mode::Execute);
        assert!(plan.is_none());
        assert_eq!(*orch.mode(), Mode::Plan); // Mode unchanged
    }

    #[test]
    fn orchestrator_accept_plan_guided() {
        let mut orch = Orchestrator::new(OrchestratorConfig {
            mode: Mode::Plan,
            ..test_config()
        });

        orch.set_current_plan("The plan content".into());
        let plan = orch.accept_plan(Mode::Guided);

        assert!(plan.is_some());
        assert_eq!(*orch.mode(), Mode::Guided);
        assert_eq!(orch.tool_count(), 10); // Same tools as Execute
    }

    #[test]
    fn set_mode_with_handler_swaps_handler() {
        let mut orch = Orchestrator::new(test_config());

        assert_eq!(*orch.mode(), Mode::Explore);

        // Switch to Guided with a new handler
        let new_handler = Arc::new(crate::ui::approval::AutoApproveHandler::always_reject())
            as Arc<dyn ApprovalHandler>;
        orch.set_mode_with_handler(Mode::Guided, Some(new_handler));

        assert_eq!(*orch.mode(), Mode::Guided);
        assert_eq!(orch.tool_count(), 10); // write + spawn_planner + get_plan tools registered
    }

    // ── Phase 5: Personality Tests ──

    #[test]
    fn orchestrator_friendly_prompt() {
        let orch = Orchestrator::new(OrchestratorConfig {
            personality: Personality::Friendly,
            ..test_config()
        });
        assert!(orch.system_prompt().contains("warm"));
        assert!(orch.system_prompt().contains("encouraging"));
    }

    #[test]
    fn orchestrator_pragmatic_prompt() {
        let orch = Orchestrator::new(OrchestratorConfig {
            personality: Personality::Pragmatic,
            ..test_config()
        });
        assert!(orch.system_prompt().contains("direct"));
        assert!(orch.system_prompt().contains("concise"));
    }

    #[test]
    fn orchestrator_none_prompt() {
        let orch = Orchestrator::new(OrchestratorConfig {
            personality: Personality::None,
            ..test_config()
        });
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
        let mut orch = Orchestrator::new(OrchestratorConfig {
            personality: Personality::Friendly,
            ..test_config()
        });
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

    // ── Auto Mode Tests ──

    #[test]
    fn orchestrator_new_auto_mode() {
        let orch = Orchestrator::new(OrchestratorConfig {
            mode: Mode::Auto,
            ..test_config()
        });
        // 5 filesystem/shell + spawn_explorer + spawn_planner + write_file + edit_file + get_plan = 10
        assert_eq!(orch.tool_count(), 10);
        assert_eq!(*orch.mode(), Mode::Auto);
        assert!(orch.system_prompt().contains("AUTO"));
        assert!(orch.system_prompt().contains("ANY shell command"));
        assert!(orch.system_prompt().contains("write_file"));
        assert!(orch.system_prompt().contains("unrestricted"));
        assert!(orch.system_prompt().contains("spawn_planner"));
    }

    #[test]
    fn orchestrator_set_mode_auto() {
        let mut orch = test_orchestrator();
        assert_eq!(*orch.mode(), Mode::Explore);

        orch.set_mode(Mode::Auto);
        assert_eq!(*orch.mode(), Mode::Auto);
        assert_eq!(orch.tool_count(), 10);
        assert!(orch.system_prompt().contains("AUTO"));

        // Switch back
        orch.set_mode(Mode::Explore);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert_eq!(orch.tool_count(), 7);
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
        let sandbox = MockSandbox::new(PathBuf::from("/tmp"));
        let prompt = Orchestrator::build_system_prompt(
            &Mode::Explore,
            std::path::Path::new("/tmp"),
            Personality::default(),
            Some(&git_ctx),
            &sandbox,
        );
        assert!(prompt.contains("Git context:"));
        assert!(prompt.contains("On branch `main`"));
        assert!(prompt.contains("src/main.rs (modified)"));
        assert!(prompt.contains("abc1234 Initial commit"));
    }

    #[test]
    fn system_prompt_no_git_context() {
        let sandbox = MockSandbox::new(PathBuf::from("/tmp"));
        let prompt = Orchestrator::build_system_prompt(
            &Mode::Explore,
            std::path::Path::new("/tmp"),
            Personality::default(),
            None,
            &sandbox,
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
        let mut orch = Orchestrator::new(OrchestratorConfig {
            working_directory: dir.path().to_path_buf(),
            ..test_config()
        });
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

        let mut orch = Orchestrator::new(OrchestratorConfig {
            working_directory: dir.path().to_path_buf(),
            ..test_config()
        });
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

        let mut orch = Orchestrator::new(OrchestratorConfig {
            working_directory: dir.path().to_path_buf(),
            ..test_config()
        });
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
        let agent = CommitAgent::new(PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(agent.agent_type(), "commit");
    }

    #[test]
    fn review_agent_accessible() {
        use crate::agent::review_agent::ReviewAgent;
        use crate::agent::Agent;
        let agent = ReviewAgent::new(PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(agent.agent_type(), "reviewer");
    }

    // ── Phase 8a: Session Tests ──

    #[test]
    fn session_set_and_get() {
        let mut orch = test_orchestrator();
        assert!(orch.session_id().is_none());
        assert!(orch.session_store().is_none());

        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let id = SessionId::new();
        orch.set_session(id.clone(), store);

        assert_eq!(orch.session_id(), Some(&id));
        assert!(orch.session_store().is_some());
    }

    #[test]
    fn emit_event_no_store_no_panic() {
        let orch = test_orchestrator();
        // Should not panic when no session store is set
        orch.emit_event(SessionEvent::UserMessage {
            content: "test".into(),
            timestamp: Utc::now(),
        });
    }

    #[test]
    fn emit_event_with_store_writes_file() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let id = SessionId::new();
        orch.set_session(id.clone(), store.clone());

        orch.emit_event(SessionEvent::UserMessage {
            content: "hello".into(),
            timestamp: Utc::now(),
        });

        let events = store.load_events(&id).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SessionEvent::UserMessage { .. }));
    }

    #[test]
    fn set_history_replaces() {
        let mut orch = test_orchestrator();
        orch.history.push(Content::user("old"));
        assert_eq!(orch.turn_count(), 1);

        orch.set_history(vec![Content::user("new1"), Content::model("new2")]);
        assert_eq!(orch.turn_count(), 2);
        assert_eq!(orch.history()[0].role.as_deref(), Some("user"));
    }

    #[test]
    fn history_accessor() {
        let mut orch = test_orchestrator();
        assert!(orch.history().is_empty());
        orch.history.push(Content::user("test"));
        assert_eq!(orch.history().len(), 1);
    }

    #[test]
    fn start_session_creates_id_and_writes() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());

        orch.start_session(store);

        assert!(orch.session_id().is_some());
        assert!(orch.session_store().is_some());

        // Verify SessionStart was written
        let id = orch.session_id().unwrap();
        let events = orch.session_store().unwrap().load_events(id).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SessionEvent::SessionStart { .. }));
    }

    #[test]
    fn fork_session_creates_new_file() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        orch.start_session(store);

        let original_id = orch.session_id().unwrap().clone();
        let new_id = orch.fork_session().unwrap().unwrap();

        assert_ne!(original_id, new_id);
        assert_eq!(orch.session_id(), Some(&new_id));

        // Both files should exist
        let store = orch.session_store().unwrap();
        assert!(store.session_path(&original_id).exists());
        assert!(store.session_path(&new_id).exists());
    }

    #[test]
    fn fork_session_without_store_returns_none() {
        let mut orch = test_orchestrator();
        let result = orch.fork_session().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn clear_history_emits_session_end_when_store_present() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        orch.start_session(store);

        let first_id = orch.session_id().unwrap().clone();
        orch.history.push(Content::user("hello"));

        orch.clear_history();

        // History cleared
        assert!(orch.history().is_empty());
        // New session ID assigned
        assert_ne!(orch.session_id().unwrap(), &first_id);

        // Original session should have SessionStart + SessionEnd
        let store = orch.session_store().unwrap();
        let events = store.load_events(&first_id).unwrap();
        assert!(events.len() >= 2);
        assert!(matches!(
            events.last().unwrap(),
            SessionEvent::SessionEnd { .. }
        ));
    }

    #[test]
    fn debug_includes_session_id() {
        let mut orch = test_orchestrator();
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        orch.start_session(store);

        let debug = format!("{:?}", orch);
        assert!(debug.contains("session_id"));
    }
}

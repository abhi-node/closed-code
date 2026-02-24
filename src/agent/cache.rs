use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::gemini::types::{
    Content, CreateCachedContentRequest, GenerateContentRequest, GenerationConfig, GeminiTool,
    ToolConfig,
};
use crate::gemini::GeminiClient;

/// Shared cache name store for sub-agents.
///
/// Maps `agent_type → (cache_name, expire_time, model)`. Sub-agents check
/// this store to reuse caches across invocations within a session.
///
/// Uses `std::sync::Mutex` (not tokio) because the critical section is just
/// HashMap ops — no async I/O under the lock. This allows calling from both
/// sync (`set_mode`) and async contexts.
pub struct SubAgentCacheManager {
    caches: Mutex<HashMap<String, CacheEntry>>,
}

struct CacheEntry {
    name: String,
    expire_time: Instant,
    model: String,
}

impl Default for SubAgentCacheManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SubAgentCacheManager {
    pub fn new() -> Self {
        Self {
            caches: Mutex::new(HashMap::new()),
        }
    }

    /// Look up a valid (non-expired, correct model) cache for this agent type.
    pub fn get(&self, agent_type: &str, model: &str) -> Option<String> {
        let caches = self.caches.lock().unwrap();
        if let Some(entry) = caches.get(agent_type) {
            // Consider valid if >5 min remaining and model matches
            let buffer = std::time::Duration::from_secs(300);
            if entry.model == model && entry.expire_time > Instant::now() + buffer {
                return Some(entry.name.clone());
            }
        }
        None
    }

    /// Store a cache entry after successful creation.
    pub fn put(&self, agent_type: &str, name: String, model: String, ttl_secs: u64) {
        let mut caches = self.caches.lock().unwrap();
        caches.insert(
            agent_type.to_string(),
            CacheEntry {
                name,
                expire_time: Instant::now() + std::time::Duration::from_secs(ttl_secs),
                model,
            },
        );
    }

    /// Remove and return all cache names (for invalidation without async delete).
    /// Server-side caches will expire via TTL.
    pub fn drain_all(&self) -> Vec<String> {
        let mut caches = self.caches.lock().unwrap();
        caches.drain().map(|(_, entry)| entry.name).collect()
    }
}

impl std::fmt::Debug for SubAgentCacheManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgentCacheManager").finish()
    }
}

const SUBAGENT_CACHE_TTL: &str = "1800s";
const SUBAGENT_CACHE_TTL_SECS: u64 = 1800;

/// Get or create a cached content resource for a sub-agent.
///
/// Called before each sub-agent's tool loop. Returns `Some(cache_name)` on
/// success, `None` if caching fails (graceful fallback to inline requests).
pub async fn ensure_subagent_cache(
    manager: &SubAgentCacheManager,
    client: &GeminiClient,
    agent_type: &str,
    system_instruction: &Content,
    tools: &Option<Vec<GeminiTool>>,
    tool_config: &Option<ToolConfig>,
) -> Option<String> {
    let model = client.model();

    // Check for existing valid cache
    if let Some(name) = manager.get(agent_type, model) {
        tracing::debug!("Sub-agent cache hit for '{}': {}", agent_type, name);
        return Some(name);
    }

    // Create new cache
    let request = CreateCachedContentRequest {
        model: format!("models/{}", model),
        system_instruction: Some(system_instruction.clone()),
        tools: tools.clone(),
        tool_config: tool_config.clone(),
        ttl: SUBAGENT_CACHE_TTL.into(),
    };

    match client.create_cached_content(&request).await {
        Ok(resp) => {
            tracing::info!(
                "Created sub-agent cache for '{}': {}",
                agent_type,
                resp.name
            );
            let name = resp.name.clone();
            manager.put(agent_type, resp.name, model.to_string(), SUBAGENT_CACHE_TTL_SECS);
            Some(name)
        }
        Err(e) => {
            tracing::warn!(
                "Sub-agent cache creation failed for '{}' (falling back to inline): {}",
                agent_type,
                e
            );
            None
        }
    }
}

/// Build a `GenerateContentRequest` for a sub-agent iteration.
///
/// When `cached_content` is `Some`, omits `system_instruction` / `tools` /
/// `tool_config` since they are covered by the cache.
pub fn build_subagent_request(
    history: &[Content],
    system_instruction: &Content,
    tools: &Option<Vec<GeminiTool>>,
    tool_config: &Option<ToolConfig>,
    cached_content: &Option<String>,
) -> GenerateContentRequest {
    if let Some(cache_name) = cached_content {
        GenerateContentRequest {
            contents: history.to_vec(),
            system_instruction: None,
            generation_config: Some(GenerationConfig {
                temperature: Some(0.7),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(8192),
            }),
            tools: None,
            tool_config: None,
            cached_content: Some(cache_name.clone()),
        }
    } else {
        GenerateContentRequest {
            contents: history.to_vec(),
            system_instruction: Some(system_instruction.clone()),
            generation_config: Some(GenerationConfig {
                temperature: Some(0.7),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(8192),
            }),
            tools: tools.clone(),
            tool_config: tool_config.clone(),
            cached_content: None,
        }
    }
}

/// Check whether an error looks like a cache-related failure (expired, invalid, not found).
pub fn is_subagent_cache_error(err: &crate::error::ClosedCodeError) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("cached") || msg.contains("cache") || msg.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_manager_get_returns_none_when_empty() {
        let mgr = SubAgentCacheManager::new();
        assert!(mgr.get("explorer", "gemini-2.0-flash").is_none());
    }

    #[test]
    fn cache_manager_put_and_get() {
        let mgr = SubAgentCacheManager::new();
        mgr.put(
            "explorer",
            "cachedContents/abc123".into(),
            "gemini-2.0-flash".into(),
            1800,
        );
        let result = mgr.get("explorer", "gemini-2.0-flash");
        assert_eq!(result, Some("cachedContents/abc123".to_string()));
    }

    #[test]
    fn cache_manager_wrong_model_returns_none() {
        let mgr = SubAgentCacheManager::new();
        mgr.put(
            "explorer",
            "cachedContents/abc123".into(),
            "gemini-2.0-flash".into(),
            1800,
        );
        assert!(mgr.get("explorer", "gemini-3.1-pro").is_none());
    }

    #[test]
    fn cache_manager_drain_all() {
        let mgr = SubAgentCacheManager::new();
        mgr.put("explorer", "cache1".into(), "model".into(), 1800);
        mgr.put("planner", "cache2".into(), "model".into(), 1800);
        let names = mgr.drain_all();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"cache1".to_string()));
        assert!(names.contains(&"cache2".to_string()));
        // After drain, get returns None
        assert!(mgr.get("explorer", "model").is_none());
    }

    #[test]
    fn build_request_with_cache() {
        let history = vec![Content::user("hello")];
        let sys = Content::system("You are an explorer");
        let req = build_subagent_request(
            &history,
            &sys,
            &None,
            &None,
            &Some("cachedContents/xyz".into()),
        );
        assert!(req.system_instruction.is_none());
        assert!(req.tools.is_none());
        assert!(req.tool_config.is_none());
        assert_eq!(req.cached_content, Some("cachedContents/xyz".to_string()));
    }

    #[test]
    fn build_request_without_cache() {
        let history = vec![Content::user("hello")];
        let sys = Content::system("You are an explorer");
        let req = build_subagent_request(&history, &sys, &None, &None, &None);
        assert!(req.system_instruction.is_some());
        assert!(req.cached_content.is_none());
    }

    #[test]
    fn cache_manager_expired_returns_none() {
        let mgr = SubAgentCacheManager::new();
        // Put with 0 TTL — immediately expired
        mgr.put("explorer", "cache1".into(), "model".into(), 0);
        // The 5-min buffer means this will definitely be expired
        assert!(mgr.get("explorer", "model").is_none());
    }
}

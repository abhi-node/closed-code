use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::gemini::GeminiClient;
use crate::sandbox::Sandbox;
use crate::ui::approval::ApprovalHandler;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::{
    FunctionCallingConfig, FunctionDeclaration, GeminiTool, ToolConfig, ToolDefinition,
};
use crate::mode::Mode;

use super::Tool;

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Panics if a tool with the same name already exists.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        assert!(
            !self.tools.contains_key(&name),
            "Duplicate tool name: {name}"
        );
        self.tools.insert(name, tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Execute a tool by name with the given arguments.
    pub async fn execute(&self, name: &str, args: Value) -> Result<Value> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ClosedCodeError::ToolNotFound {
                name: name.to_string(),
            })?;

        tracing::debug!("Executing tool '{}' with args: {}", name, args);

        tool.execute(args).await
    }

    /// Get function declarations for tools available in the given mode.
    pub fn declarations_for_mode(&self, mode: &Mode) -> Vec<FunctionDeclaration> {
        self.tools
            .values()
            .filter(|tool| tool.available_modes().contains(mode))
            .map(|tool| tool.declaration())
            .collect()
    }

    /// Generate the `tools` array for a Gemini API request.
    /// Returns None if no tools are available for the given mode.
    pub fn to_gemini_tools(&self, mode: &Mode) -> Option<Vec<GeminiTool>> {
        let declarations = self.declarations_for_mode(mode);
        if declarations.is_empty() {
            None
        } else {
            Some(vec![GeminiTool::Functions(ToolDefinition {
                function_declarations: declarations,
            })])
        }
    }

    /// Generate the default `tool_config` for a Gemini API request.
    pub fn tool_config() -> ToolConfig {
        ToolConfig {
            function_calling_config: FunctionCallingConfig {
                mode: "AUTO".into(),
            },
        }
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Create a ToolRegistry with all Phase 2 tools registered.
pub fn create_default_registry(
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_filesystem_tools(&mut registry, working_directory, false, sandbox);
    registry
}

/// Register the common filesystem + shell tools into a registry.
fn register_filesystem_tools(
    registry: &mut ToolRegistry,
    working_directory: PathBuf,
    bypass_shell_allowlist: bool,
    sandbox: Arc<dyn Sandbox>,
) {
    registry.register(Box::new(super::filesystem::ReadFileTool::new(
        working_directory.clone(),
    )));
    registry.register(Box::new(super::filesystem::ListDirectoryTool::new(
        working_directory.clone(),
    )));
    registry.register(Box::new(super::filesystem::SearchFilesTool::new(
        working_directory.clone(),
    )));
    registry.register(Box::new(super::filesystem::GrepTool::new(
        working_directory.clone(),
    )));
    if bypass_shell_allowlist {
        registry.register(Box::new(
            super::shell::ShellCommandTool::with_bypass_allowlist(working_directory, sandbox),
        ));
    } else {
        registry.register(Box::new(super::shell::ShellCommandTool::new(
            working_directory,
            sandbox,
        )));
    }
}

/// Create a ToolRegistry for Explorer sub-agents.
/// Includes filesystem tools + create_report. No spawn tools (terminal).
pub fn create_subagent_registry(
    working_directory: PathBuf,
    sandbox: Arc<dyn Sandbox>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_filesystem_tools(&mut registry, working_directory, false, sandbox);
    registry.register(Box::new(super::report::CreateReportTool::new()));
    registry
}

/// Create a ToolRegistry for the orchestrator.
/// Includes filesystem + spawn tools based on mode.
///
/// In Execute mode, `approval_handler` must be `Some` to register write tools.
/// In other modes, `approval_handler` is ignored (write tools are not registered).
pub fn create_orchestrator_registry(
    working_directory: PathBuf,
    mode: &Mode,
    client: Arc<GeminiClient>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    sandbox: Arc<dyn Sandbox>,
    protected_paths: Vec<String>,
    event_tx: Option<
        tokio::sync::mpsc::UnboundedSender<crate::agent::orchestrator::OrchestratorEvent>,
    >,
) -> ToolRegistry {
    let bypass_shell = matches!(mode, Mode::Auto);
    let mut registry = ToolRegistry::new();
    register_filesystem_tools(
        &mut registry,
        working_directory.clone(),
        bypass_shell,
        sandbox.clone(),
    );

    // Spawn tools (mode-dependent)
    match mode {
        Mode::Explore => {
            registry.register(Box::new(
                super::spawn::SpawnExplorerTool::new(client, working_directory, sandbox)
                    .with_event_tx(event_tx),
            ));
        }
        Mode::Plan => {
            registry.register(Box::new(
                super::spawn::SpawnExplorerTool::new(
                    client.clone(),
                    working_directory.clone(),
                    sandbox.clone(),
                )
                .with_event_tx(event_tx.clone()),
            ));
            registry.register(Box::new(
                super::spawn::SpawnPlannerTool::new(
                    client.clone(),
                    working_directory.clone(),
                    sandbox,
                )
                .with_event_tx(event_tx),
            ));
            registry.register(Box::new(super::spawn::SpawnWebSearchTool::new(
                client,
                working_directory,
            )));
        }
        Mode::Guided | Mode::Execute | Mode::Auto => {
            registry.register(Box::new(
                super::spawn::SpawnExplorerTool::new(
                    client.clone(),
                    working_directory.clone(),
                    sandbox.clone(),
                )
                .with_event_tx(event_tx.clone()),
            ));
            registry.register(Box::new(
                super::spawn::SpawnPlannerTool::new(client, working_directory.clone(), sandbox)
                    .with_event_tx(event_tx),
            ));
            // Write tools — Guided, Execute, and Auto modes
            if let Some(handler) = approval_handler {
                registry.register(Box::new(super::file_write::WriteFileTool::new(
                    working_directory.clone(),
                    handler.clone(),
                    protected_paths.clone(),
                )));
                registry.register(Box::new(super::file_edit::EditFileTool::new(
                    working_directory,
                    handler,
                    protected_paths,
                )));
            }
        }
    }

    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::types::Parameters;
    use crate::sandbox::mock::MockSandbox;
    use async_trait::async_trait;
    use serde_json::json;

    fn mock_sandbox() -> Arc<dyn Sandbox> {
        Arc::new(MockSandbox::new(PathBuf::from("/tmp")))
    }

    #[derive(Debug)]
    struct MockTool {
        tool_name: String,
        modes: Vec<Mode>,
    }

    impl MockTool {
        fn new(name: &str) -> Self {
            Self {
                tool_name: name.to_string(),
                modes: vec![Mode::Explore, Mode::Plan, Mode::Execute, Mode::Auto],
            }
        }

        fn with_modes(name: &str, modes: Vec<Mode>) -> Self {
            Self {
                tool_name: name.to_string(),
                modes,
            }
        }
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            "A mock tool for testing"
        }

        fn declaration(&self) -> crate::gemini::types::FunctionDeclaration {
            crate::gemini::types::FunctionDeclaration {
                name: self.tool_name.clone(),
                description: self.description().into(),
                parameters: Parameters {
                    schema_type: "object".into(),
                    properties: serde_json::Map::new(),
                    required: None,
                },
            }
        }

        async fn execute(&self, _args: Value) -> Result<Value> {
            Ok(json!({"ok": true, "tool": self.tool_name}))
        }

        fn available_modes(&self) -> Vec<Mode> {
            self.modes.clone()
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool::new("test_tool")));
        assert!(registry.get("test_tool").is_some());
        assert_eq!(registry.get("test_tool").unwrap().name(), "test_tool");
    }

    #[test]
    fn registry_get_nonexistent() {
        let registry = ToolRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn registry_execute_success() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool::new("test_tool")));
        let result = registry.execute("test_tool", json!({})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["tool"], "test_tool");
    }

    #[tokio::test]
    async fn registry_execute_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", json!({})).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not found in registry"));
    }

    #[test]
    #[should_panic(expected = "Duplicate tool name")]
    fn registry_duplicate_name_panics() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool::new("dupe")));
        registry.register(Box::new(MockTool::new("dupe")));
    }

    #[test]
    fn registry_declarations_for_mode() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool::new("all_modes")));
        registry.register(Box::new(MockTool::with_modes(
            "execute_only",
            vec![Mode::Execute],
        )));

        let explore_decls = registry.declarations_for_mode(&Mode::Explore);
        let execute_decls = registry.declarations_for_mode(&Mode::Execute);

        assert_eq!(explore_decls.len(), 1);
        assert_eq!(explore_decls[0].name, "all_modes");

        assert_eq!(execute_decls.len(), 2);
    }

    #[test]
    fn registry_to_gemini_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool::new("tool1")));
        registry.register(Box::new(MockTool::new("tool2")));

        let tools = registry.to_gemini_tools(&Mode::Explore);
        assert!(tools.is_some());
        let tools = tools.unwrap();
        assert_eq!(tools.len(), 1); // one GeminiTool::Functions wrapper
        match &tools[0] {
            GeminiTool::Functions(def) => assert_eq!(def.function_declarations.len(), 2),
            _ => panic!("Expected GeminiTool::Functions"),
        }
    }

    #[test]
    fn registry_to_gemini_tools_empty() {
        let registry = ToolRegistry::new();
        assert!(registry.to_gemini_tools(&Mode::Explore).is_none());
    }

    #[test]
    fn registry_tool_config() {
        let config = ToolRegistry::tool_config();
        assert_eq!(config.function_calling_config.mode, "AUTO");
    }

    #[test]
    fn registry_len() {
        let mut registry = ToolRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
        registry.register(Box::new(MockTool::new("a")));
        registry.register(Box::new(MockTool::new("b")));
        registry.register(Box::new(MockTool::new("c")));
        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
    }

    #[test]
    fn create_default_registry_has_all_tools() {
        let registry = create_default_registry(PathBuf::from("/tmp"), mock_sandbox());
        assert_eq!(registry.len(), 5);
        assert!(registry.get("read_file").is_some());
        assert!(registry.get("list_directory").is_some());
        assert!(registry.get("search_files").is_some());
        assert!(registry.get("grep").is_some());
        assert!(registry.get("shell").is_some());
    }

    #[test]
    fn registry_debug_format() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool::new("test_tool")));
        let debug = format!("{:?}", registry);
        assert!(debug.contains("ToolRegistry"));
        assert!(debug.contains("test_tool"));
    }

    // ── Phase 3 Registry Factory Tests ──

    #[test]
    fn create_orchestrator_registry_explore_mode() {
        let client = Arc::new(crate::gemini::GeminiClient::new(
            "key".into(),
            "model".into(),
        ));
        let registry = create_orchestrator_registry(
            PathBuf::from("/tmp"),
            &Mode::Explore,
            client,
            None,
            mock_sandbox(),
            vec![],
            None,
        );
        // 5 filesystem/shell + spawn_explorer = 6
        assert_eq!(registry.len(), 6);
        assert!(registry.get("spawn_explorer").is_some());
        assert!(registry.get("spawn_planner").is_none());
        assert!(registry.get("spawn_web_search").is_none());
    }

    #[test]
    fn create_orchestrator_registry_plan_mode() {
        let client = Arc::new(crate::gemini::GeminiClient::new(
            "key".into(),
            "model".into(),
        ));
        let registry = create_orchestrator_registry(
            PathBuf::from("/tmp"),
            &Mode::Plan,
            client,
            None,
            mock_sandbox(),
            vec![],
            None,
        );
        // 5 filesystem/shell + spawn_explorer + spawn_planner + spawn_web_search = 8
        assert_eq!(registry.len(), 8);
        assert!(registry.get("spawn_explorer").is_some());
        assert!(registry.get("spawn_planner").is_some());
        assert!(registry.get("spawn_web_search").is_some());
    }

    // ── Phase 4 Registry Factory Tests ──

    #[test]
    fn create_orchestrator_registry_execute_mode_with_handler() {
        let client = Arc::new(crate::gemini::GeminiClient::new(
            "key".into(),
            "model".into(),
        ));
        let handler = Arc::new(crate::ui::approval::AutoApproveHandler::always_approve())
            as Arc<dyn crate::ui::approval::ApprovalHandler>;
        let registry = create_orchestrator_registry(
            PathBuf::from("/tmp"),
            &Mode::Execute,
            client,
            Some(handler),
            mock_sandbox(),
            vec![],
            None,
        );
        // 5 filesystem/shell + spawn_explorer + spawn_planner + write_file + edit_file = 9
        assert_eq!(registry.len(), 9);
        assert!(registry.get("write_file").is_some());
        assert!(registry.get("edit_file").is_some());
        assert!(registry.get("spawn_explorer").is_some());
        assert!(registry.get("spawn_planner").is_some());
    }

    #[test]
    fn create_orchestrator_registry_execute_mode_without_handler() {
        let client = Arc::new(crate::gemini::GeminiClient::new(
            "key".into(),
            "model".into(),
        ));
        let registry = create_orchestrator_registry(
            PathBuf::from("/tmp"),
            &Mode::Execute,
            client,
            None,
            mock_sandbox(),
            vec![],
            None,
        );
        // No write tools, but spawn tools: 5 filesystem/shell + spawn_explorer + spawn_planner = 7
        assert_eq!(registry.len(), 7);
        assert!(registry.get("write_file").is_none());
        assert!(registry.get("edit_file").is_none());
    }

    #[test]
    fn create_orchestrator_registry_explore_ignores_handler() {
        let client = Arc::new(crate::gemini::GeminiClient::new(
            "key".into(),
            "model".into(),
        ));
        let handler = Arc::new(crate::ui::approval::AutoApproveHandler::always_approve())
            as Arc<dyn crate::ui::approval::ApprovalHandler>;
        let registry = create_orchestrator_registry(
            PathBuf::from("/tmp"),
            &Mode::Explore,
            client,
            Some(handler),
            mock_sandbox(),
            vec![],
            None,
        );
        // Explore mode: no write tools regardless of handler
        assert_eq!(registry.len(), 6);
        assert!(registry.get("write_file").is_none());
    }

    #[test]
    fn create_orchestrator_registry_guided_mode() {
        let client = Arc::new(crate::gemini::GeminiClient::new(
            "key".into(),
            "model".into(),
        ));
        let handler = Arc::new(crate::ui::approval::AutoApproveHandler::always_approve())
            as Arc<dyn crate::ui::approval::ApprovalHandler>;
        let registry = create_orchestrator_registry(
            PathBuf::from("/tmp"),
            &Mode::Guided,
            client,
            Some(handler),
            mock_sandbox(),
            vec![],
            None,
        );
        // 5 filesystem/shell + spawn_explorer + spawn_planner + write_file + edit_file = 9
        assert_eq!(registry.len(), 9);
        assert!(registry.get("write_file").is_some());
        assert!(registry.get("edit_file").is_some());
        assert!(registry.get("spawn_explorer").is_some());
        assert!(registry.get("spawn_planner").is_some());
        // Guided does not bypass shell allowlist
        assert!(registry.get("shell").is_some());
    }

    #[test]
    fn create_subagent_registry_has_tools() {
        let registry = create_subagent_registry(PathBuf::from("/tmp"), mock_sandbox());
        // 5 filesystem/shell + create_report = 6
        assert_eq!(registry.len(), 6);
        assert!(registry.get("create_report").is_some());
        assert!(registry.get("spawn_explorer").is_none());
    }

    #[test]
    fn create_orchestrator_registry_auto_mode() {
        let client = Arc::new(crate::gemini::GeminiClient::new(
            "key".into(),
            "model".into(),
        ));
        let handler = Arc::new(crate::ui::approval::AutoApproveHandler::always_approve())
            as Arc<dyn crate::ui::approval::ApprovalHandler>;
        let registry = create_orchestrator_registry(
            PathBuf::from("/tmp"),
            &Mode::Auto,
            client,
            Some(handler),
            mock_sandbox(),
            vec![],
            None,
        );
        // 5 filesystem/shell + spawn_explorer + spawn_planner + write_file + edit_file = 9
        assert_eq!(registry.len(), 9);
        assert!(registry.get("write_file").is_some());
        assert!(registry.get("edit_file").is_some());
        assert!(registry.get("spawn_explorer").is_some());
        assert!(registry.get("spawn_planner").is_some());
        assert!(registry.get("shell").is_some());
    }
}

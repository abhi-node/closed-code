use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::{FunctionCallingConfig, FunctionDeclaration, ToolConfig, ToolDefinition};
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
    pub fn to_gemini_tools(&self, mode: &Mode) -> Option<Vec<ToolDefinition>> {
        let declarations = self.declarations_for_mode(mode);
        if declarations.is_empty() {
            None
        } else {
            Some(vec![ToolDefinition {
                function_declarations: declarations,
            }])
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
pub fn create_default_registry(working_directory: PathBuf) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
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
    registry.register(Box::new(super::shell::ShellCommandTool::new(
        working_directory,
    )));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::types::Parameters;
    use async_trait::async_trait;
    use serde_json::json;

    #[derive(Debug)]
    struct MockTool {
        tool_name: String,
        modes: Vec<Mode>,
    }

    impl MockTool {
        fn new(name: &str) -> Self {
            Self {
                tool_name: name.to_string(),
                modes: vec![Mode::Explore, Mode::Plan, Mode::Execute],
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
        assert_eq!(tools.len(), 1); // one ToolDefinition wrapper
        assert_eq!(tools[0].function_declarations.len(), 2);
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
        let registry = create_default_registry(PathBuf::from("/tmp"));
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
}

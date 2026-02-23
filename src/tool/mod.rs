use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::fmt::Debug;

use crate::error::Result;
use crate::gemini::types::{FunctionDeclaration, Parameters};
use crate::mode::Mode;

pub mod file_edit;
pub mod file_write;
pub mod filesystem;
pub mod registry;
pub mod report;
pub mod shell;
pub mod spawn;

/// A tool that the LLM can invoke via Gemini function calling.
#[async_trait]
pub trait Tool: Send + Sync + Debug {
    /// Unique name matching the Gemini function declaration.
    fn name(&self) -> &str;

    /// Human-readable description for the Gemini API.
    fn description(&self) -> &str;

    /// Generate the Gemini FunctionDeclaration for this tool.
    fn declaration(&self) -> FunctionDeclaration;

    /// Execute the tool with the given arguments (from Gemini's functionCall.args).
    /// Returns a JSON value that will be sent back as functionResponse.response.
    async fn execute(&self, args: Value) -> Result<Value>;

    /// Which modes this tool is available in.
    /// Default: all modes (Explore, Plan, Guided, Execute, Auto).
    fn available_modes(&self) -> Vec<Mode> {
        vec![
            Mode::Explore,
            Mode::Plan,
            Mode::Guided,
            Mode::Execute,
            Mode::Auto,
        ]
    }
}

/// Builder for FunctionDeclaration parameter schemas.
pub struct ParamBuilder {
    properties: Map<String, Value>,
    required: Vec<String>,
}

impl ParamBuilder {
    pub fn new() -> Self {
        Self {
            properties: Map::new(),
            required: Vec::new(),
        }
    }

    /// Add a string parameter.
    pub fn string(mut self, name: &str, description: &str, required: bool) -> Self {
        self.properties.insert(
            name.into(),
            json!({
                "type": "string",
                "description": description,
            }),
        );
        if required {
            self.required.push(name.into());
        }
        self
    }

    /// Add an integer parameter.
    pub fn integer(mut self, name: &str, description: &str, required: bool) -> Self {
        self.properties.insert(
            name.into(),
            json!({
                "type": "integer",
                "description": description,
            }),
        );
        if required {
            self.required.push(name.into());
        }
        self
    }

    /// Add a boolean parameter.
    pub fn boolean(mut self, name: &str, description: &str, required: bool) -> Self {
        self.properties.insert(
            name.into(),
            json!({
                "type": "boolean",
                "description": description,
            }),
        );
        if required {
            self.required.push(name.into());
        }
        self
    }

    /// Build into Parameters.
    pub fn build(self) -> Parameters {
        Parameters {
            schema_type: "object".into(),
            properties: self.properties,
            required: if self.required.is_empty() {
                None
            } else {
                Some(self.required)
            },
        }
    }
}

impl Default for ParamBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Default hardcoded protected paths.
const DEFAULT_PROTECTED_PATHS: &[&str] = &[".git", ".closed-code", ".env"];

/// Default protected file extensions (matched case-insensitively).
const DEFAULT_PROTECTED_EXTENSIONS: &[&str] = &[".pem", ".key"];

/// Check if a path is protected from modification.
///
/// A path is protected if it matches any of:
/// - Hardcoded paths: .git/, .closed-code/, .env
/// - Hardcoded extensions: *.pem, *.key
/// - User-configured additional protected paths from config.toml
pub fn is_protected_path(path: &str, additional_paths: &[String]) -> bool {
    let normalized = path.replace('\\', "/");

    // Check hardcoded directory/file paths
    for protected in DEFAULT_PROTECTED_PATHS {
        if normalized == *protected || normalized.starts_with(&format!("{protected}/")) {
            return true;
        }
    }

    // Check hardcoded extensions
    let lower = normalized.to_lowercase();
    for ext in DEFAULT_PROTECTED_EXTENSIONS {
        if lower.ends_with(ext) {
            return true;
        }
    }

    // Check user-configured additional paths
    for additional in additional_paths {
        let additional_normalized = additional.replace('\\', "/");
        if normalized == additional_normalized
            || normalized.starts_with(&format!("{additional_normalized}/"))
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_builder_empty() {
        let params = ParamBuilder::new().build();
        assert_eq!(params.schema_type, "object");
        assert!(params.properties.is_empty());
        assert!(params.required.is_none());
    }

    #[test]
    fn param_builder_single_required_string() {
        let params = ParamBuilder::new()
            .string("path", "File path", true)
            .build();
        assert!(params.properties.contains_key("path"));
        assert_eq!(params.properties["path"]["type"], "string");
        assert_eq!(params.properties["path"]["description"], "File path");
        assert_eq!(params.required, Some(vec!["path".to_string()]));
    }

    #[test]
    fn param_builder_multiple_params() {
        let params = ParamBuilder::new()
            .string("name", "Name", true)
            .integer("count", "Count", false)
            .boolean("verbose", "Verbose", false)
            .build();
        assert_eq!(params.properties.len(), 3);
        assert_eq!(params.properties["name"]["type"], "string");
        assert_eq!(params.properties["count"]["type"], "integer");
        assert_eq!(params.properties["verbose"]["type"], "boolean");
        assert_eq!(params.required, Some(vec!["name".to_string()]));
    }

    #[test]
    fn param_builder_no_required_fields() {
        let params = ParamBuilder::new()
            .string("opt1", "Optional 1", false)
            .integer("opt2", "Optional 2", false)
            .build();
        assert!(params.required.is_none());
    }

    #[test]
    fn param_builder_all_required_fields() {
        let params = ParamBuilder::new()
            .string("a", "A", true)
            .integer("b", "B", true)
            .boolean("c", "C", true)
            .build();
        let required = params.required.unwrap();
        assert_eq!(required.len(), 3);
        assert!(required.contains(&"a".to_string()));
        assert!(required.contains(&"b".to_string()));
        assert!(required.contains(&"c".to_string()));
    }

    #[test]
    fn param_builder_serialization() {
        let params = ParamBuilder::new()
            .string("path", "File path", true)
            .integer("line", "Line number", false)
            .build();
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["type"], "object");
        assert_eq!(json["properties"]["path"]["type"], "string");
        assert_eq!(json["properties"]["line"]["type"], "integer");
        assert_eq!(json["required"], serde_json::json!(["path"]));
    }

    // ── Protected Path Tests (Phase 7) ──

    #[test]
    fn protected_path_git() {
        assert!(is_protected_path(".git", &[]));
        assert!(is_protected_path(".git/config", &[]));
        assert!(is_protected_path(".git/hooks/pre-commit", &[]));
    }

    #[test]
    fn protected_path_closed_code() {
        assert!(is_protected_path(".closed-code", &[]));
        assert!(is_protected_path(".closed-code/config.toml", &[]));
    }

    #[test]
    fn protected_path_env() {
        assert!(is_protected_path(".env", &[]));
    }

    #[test]
    fn protected_path_pem_extension() {
        assert!(is_protected_path("secrets/api.pem", &[]));
        assert!(is_protected_path("CERT.PEM", &[]));
    }

    #[test]
    fn protected_path_key_extension() {
        assert!(is_protected_path("server.key", &[]));
        assert!(is_protected_path("SSL.KEY", &[]));
    }

    #[test]
    fn not_protected_github() {
        assert!(!is_protected_path(".github/workflows/ci.yml", &[]));
    }

    #[test]
    fn not_protected_gitignore() {
        assert!(!is_protected_path(".gitignore", &[]));
        assert!(!is_protected_path("src/.gitignore", &[]));
    }

    #[test]
    fn not_protected_src_file() {
        assert!(!is_protected_path("src/main.rs", &[]));
    }

    #[test]
    fn not_protected_env_example() {
        // .env.example does NOT equal ".env" and does NOT start with ".env/"
        assert!(!is_protected_path(".env.example", &[]));
    }

    #[test]
    fn additional_paths_custom() {
        let additional = vec![".secrets".to_string(), "credentials.json".to_string()];
        assert!(is_protected_path(".secrets", &additional));
        assert!(is_protected_path(".secrets/api_key.txt", &additional));
        assert!(is_protected_path("credentials.json", &additional));
        assert!(!is_protected_path("src/main.rs", &additional));
    }

    #[test]
    fn additional_paths_empty() {
        assert!(is_protected_path(".git", &[]));
        assert!(!is_protected_path("src/main.rs", &[]));
    }

    #[test]
    fn additional_paths_nested() {
        let additional = vec!["config/secrets".to_string()];
        assert!(is_protected_path("config/secrets", &additional));
        assert!(is_protected_path(
            "config/secrets/deep/file.txt",
            &additional
        ));
        assert!(!is_protected_path("config/other.txt", &additional));
    }
}

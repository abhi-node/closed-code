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
    /// Default: all modes (Explore, Plan, Execute).
    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Explore, Mode::Plan, Mode::Execute]
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
}

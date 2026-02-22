use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt;

// ── Request Types ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

// ── Tool Definition Types (Phase 2) ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Parameters,
}

#[derive(Debug, Clone, Serialize)]
pub struct Parameters {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig {
    pub function_calling_config: FunctionCallingConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionCallingConfig {
    pub mode: String,
}

// ── Part Enum (custom deserialization) ──

#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    FunctionCall {
        name: String,
        args: Value,
        /// Thinking models (Gemini 3.x) attach a thought signature to function calls.
        /// Must be preserved and echoed back in conversation history.
        thought_signature: Option<String>,
    },
    FunctionResponse {
        name: String,
        response: Value,
    },
    InlineData {
        mime_type: String,
        data: String,
    },
}

impl Serialize for Part {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            Part::Text(text) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("text", text)?;
                map.end()
            }
            Part::FunctionCall {
                name,
                args,
                thought_signature,
            } => {
                let count = if thought_signature.is_some() { 2 } else { 1 };
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry(
                    "functionCall",
                    &serde_json::json!({"name": name, "args": args}),
                )?;
                if let Some(sig) = thought_signature {
                    map.serialize_entry("thoughtSignature", sig)?;
                }
                map.end()
            }
            Part::FunctionResponse { name, response } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(
                    "functionResponse",
                    &serde_json::json!({"name": name, "response": response}),
                )?;
                map.end()
            }
            Part::InlineData { mime_type, data } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(
                    "inlineData",
                    &serde_json::json!({"mimeType": mime_type, "data": data}),
                )?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Part {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(PartVisitor)
    }
}

struct PartVisitor;

impl<'de> Visitor<'de> for PartVisitor {
    type Value = Part;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a Part object with text, functionCall, functionResponse, or inlineData")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        // Gemini thinking models (e.g. gemini-3.1-pro-preview) add extra fields
        // like `thoughtSignature` to Part objects. We must consume ALL map entries
        // to avoid serde_json "expected closing brace" errors from unconsumed fields.
        // thoughtSignature must be preserved on FunctionCall parts and echoed back.
        let mut result: Option<Part> = None;
        let mut thought_signature: Option<String> = None;

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "text" => {
                    let text: String = map.next_value()?;
                    result = Some(Part::Text(text));
                }
                "functionCall" => {
                    let call: FunctionCallRaw = map.next_value()?;
                    result = Some(Part::FunctionCall {
                        name: call.name,
                        args: call.args,
                        thought_signature: None, // filled in after loop
                    });
                }
                "functionResponse" => {
                    let resp: FunctionResponseRaw = map.next_value()?;
                    result = Some(Part::FunctionResponse {
                        name: resp.name,
                        response: resp.response,
                    });
                }
                "inlineData" => {
                    let data: InlineDataRaw = map.next_value()?;
                    result = Some(Part::InlineData {
                        mime_type: data.mime_type,
                        data: data.data,
                    });
                }
                "thoughtSignature" => {
                    thought_signature = Some(map.next_value()?);
                }
                _ => {
                    // Skip other unknown fields
                    let _: Value = map.next_value()?;
                }
            }
        }

        // Attach thought_signature to FunctionCall if both are present
        if let Some(sig) = thought_signature {
            if let Some(Part::FunctionCall {
                ref mut thought_signature,
                ..
            }) = result
            {
                *thought_signature = Some(sig);
            }
        }

        result.ok_or_else(|| {
            de::Error::custom(
                "Part object contained no recognized field \
                 (expected text, functionCall, functionResponse, or inlineData)",
            )
        })
    }
}

#[derive(Deserialize)]
struct FunctionCallRaw {
    name: String,
    args: Value,
}

#[derive(Deserialize)]
struct FunctionResponseRaw {
    name: String,
    response: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineDataRaw {
    mime_type: String,
    data: String,
}

// ── Response Types ──

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentResponse {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    pub usage_metadata: Option<UsageMetadata>,
    pub model_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub content: Option<Content>,
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub safety_ratings: Vec<SafetyRating>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    pub prompt_token_count: Option<u32>,
    pub candidates_token_count: Option<u32>,
    pub total_token_count: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SafetyRating {
    pub category: String,
    pub probability: String,
}

// ── Helper constructors ──

impl Content {
    pub fn user(text: &str) -> Self {
        Content {
            role: Some("user".into()),
            parts: vec![Part::Text(text.into())],
        }
    }

    pub fn model(text: &str) -> Self {
        Content {
            role: Some("model".into()),
            parts: vec![Part::Text(text.into())],
        }
    }

    pub fn system(text: &str) -> Self {
        Content {
            role: None,
            parts: vec![Part::Text(text.into())],
        }
    }

    /// Build a Content with function response parts (role: "user").
    pub fn function_responses(responses: Vec<Part>) -> Self {
        Content {
            role: Some("user".into()),
            parts: responses,
        }
    }
}

impl GenerateContentResponse {
    /// Extract the text from the first candidate's first text part.
    pub fn text(&self) -> Option<&str> {
        self.candidates
            .first()
            .and_then(|c| c.content.as_ref())
            .and_then(|content| content.parts.first())
            .and_then(|part| match part {
                Part::Text(t) => Some(t.as_str()),
                _ => None,
            })
    }

    /// Extract all function call parts from the first candidate.
    pub fn function_calls(&self) -> Vec<&Part> {
        self.candidates
            .first()
            .and_then(|c| c.content.as_ref())
            .map(|content| {
                content
                    .parts
                    .iter()
                    .filter(|p| matches!(p, Part::FunctionCall { .. }))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether this response contains any function calls.
    pub fn has_function_calls(&self) -> bool {
        !self.function_calls().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_text_part() {
        let json = r#"{"text": "hello world"}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        assert!(matches!(part, Part::Text(ref s) if s == "hello world"));
    }

    #[test]
    fn deserialize_function_call_part() {
        let json = r#"{"functionCall": {"name": "read_file", "args": {"path": "/tmp/test"}}}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        match part {
            Part::FunctionCall {
                name,
                args,
                thought_signature,
            } => {
                assert_eq!(name, "read_file");
                assert_eq!(args["path"], "/tmp/test");
                assert!(thought_signature.is_none());
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn deserialize_function_response_part() {
        let json =
            r#"{"functionResponse": {"name": "read_file", "response": {"content": "data"}}}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        match part {
            Part::FunctionResponse { name, response } => {
                assert_eq!(name, "read_file");
                assert_eq!(response["content"], "data");
            }
            _ => panic!("Expected FunctionResponse"),
        }
    }

    #[test]
    fn deserialize_inline_data_part() {
        let json = r#"{"inlineData": {"mimeType": "image/png", "data": "base64data"}}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        match part {
            Part::InlineData { mime_type, data } => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(data, "base64data");
            }
            _ => panic!("Expected InlineData"),
        }
    }

    #[test]
    fn deserialize_unknown_part_only_fails() {
        let json = r#"{"unknownField": "value"}"#;
        let result = serde_json::from_str::<Part>(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no recognized field"));
    }

    #[test]
    fn deserialize_part_with_thought_signature() {
        // Gemini thinking models add thoughtSignature alongside the main field
        let json = r#"{"functionCall": {"name": "grep", "args": {"pattern": "test"}}, "thoughtSignature": "EpgJCpUJAb4+base64data..."}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        match part {
            Part::FunctionCall {
                name,
                args,
                thought_signature,
            } => {
                assert_eq!(name, "grep");
                assert_eq!(args["pattern"], "test");
                assert_eq!(
                    thought_signature.as_deref(),
                    Some("EpgJCpUJAb4+base64data...")
                );
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn deserialize_part_with_unknown_field_before_known() {
        // Unknown field appears before the recognized field
        let json = r#"{"thoughtSignature": "abc123", "text": "Hello"}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        assert!(matches!(part, Part::Text(ref s) if s == "Hello"));
    }

    #[test]
    fn deserialize_part_with_multiple_unknown_fields() {
        let json = r#"{"extra1": 42, "text": "Hello", "extra2": true}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        assert!(matches!(part, Part::Text(ref s) if s == "Hello"));
    }

    #[test]
    fn serialize_text_part() {
        let part = Part::Text("hello".into());
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["text"], "hello");
    }

    #[test]
    fn serialize_function_call_part() {
        let part = Part::FunctionCall {
            name: "test_fn".into(),
            args: serde_json::json!({"key": "value"}),
            thought_signature: None,
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["functionCall"]["name"], "test_fn");
        assert_eq!(json["functionCall"]["args"]["key"], "value");
    }

    #[test]
    fn part_roundtrip() {
        let original = Part::Text("roundtrip test".into());
        let json_str = serde_json::to_string(&original).unwrap();
        let deserialized: Part = serde_json::from_str(&json_str).unwrap();
        assert!(matches!(deserialized, Part::Text(ref s) if s == "roundtrip test"));
    }

    #[test]
    fn serialize_function_call_with_thought_signature() {
        let part = Part::FunctionCall {
            name: "grep".into(),
            args: serde_json::json!({"pattern": "test"}),
            thought_signature: Some("abc123sig".into()),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["functionCall"]["name"], "grep");
        assert_eq!(json["thoughtSignature"], "abc123sig");
    }

    #[test]
    fn serialize_function_call_without_thought_signature() {
        let part = Part::FunctionCall {
            name: "grep".into(),
            args: serde_json::json!({"pattern": "test"}),
            thought_signature: None,
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["functionCall"]["name"], "grep");
        assert!(json.get("thoughtSignature").is_none());
    }

    #[test]
    fn thought_signature_roundtrip() {
        let original = Part::FunctionCall {
            name: "read_file".into(),
            args: serde_json::json!({"path": "/tmp"}),
            thought_signature: Some("EpgJCpUJAb4+base64".into()),
        };
        let json_str = serde_json::to_string(&original).unwrap();
        let deserialized: Part = serde_json::from_str(&json_str).unwrap();
        match deserialized {
            Part::FunctionCall {
                name,
                thought_signature,
                ..
            } => {
                assert_eq!(name, "read_file");
                assert_eq!(thought_signature.as_deref(), Some("EpgJCpUJAb4+base64"));
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn content_user_constructor() {
        let content = Content::user("hello");
        assert_eq!(content.role.as_deref(), Some("user"));
        assert_eq!(content.parts.len(), 1);
    }

    #[test]
    fn content_model_constructor() {
        let content = Content::model("response");
        assert_eq!(content.role.as_deref(), Some("model"));
    }

    #[test]
    fn content_system_constructor() {
        let content = Content::system("system prompt");
        assert!(content.role.is_none());
    }

    #[test]
    fn response_text_extraction() {
        let response = GenerateContentResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".into()),
                    parts: vec![Part::Text("extracted text".into())],
                }),
                finish_reason: Some("STOP".into()),
                safety_ratings: vec![],
            }],
            usage_metadata: None,
            model_version: None,
        };
        assert_eq!(response.text(), Some("extracted text"));
    }

    #[test]
    fn response_text_empty_candidates() {
        let response = GenerateContentResponse {
            candidates: vec![],
            usage_metadata: None,
            model_version: None,
        };
        assert_eq!(response.text(), None);
    }

    #[test]
    fn deserialize_full_response() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Hello!"}]
                },
                "finishReason": "STOP",
                "safetyRatings": [
                    {"category": "HARM_CATEGORY_HARASSMENT", "probability": "NEGLIGIBLE"}
                ]
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15
            },
            "modelVersion": "gemini-3.1-pro-preview"
        }"#;
        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text(), Some("Hello!"));
        assert_eq!(response.candidates[0].finish_reason.as_deref(), Some("STOP"));
        let usage = response.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(10));
        assert_eq!(usage.candidates_token_count, Some(5));
        assert_eq!(usage.total_token_count, Some(15));
    }

    #[test]
    fn serialize_request_omits_none_fields() {
        let request = GenerateContentRequest {
            contents: vec![Content::user("test")],
            system_instruction: None,
            generation_config: None,
            tools: None,
            tool_config: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("systemInstruction").is_none());
        assert!(json.get("generationConfig").is_none());
        assert!(json.get("tools").is_none());
        assert!(json.get("toolConfig").is_none());
    }

    #[test]
    fn serialize_request_includes_system_instruction() {
        let request = GenerateContentRequest {
            contents: vec![Content::user("test")],
            system_instruction: Some(Content::system("be helpful")),
            generation_config: Some(GenerationConfig {
                temperature: Some(1.0),
                top_p: None,
                top_k: None,
                max_output_tokens: Some(8192),
            }),
            tools: None,
            tool_config: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("systemInstruction").is_some());
        assert!(json.get("generationConfig").is_some());
        assert_eq!(json["generationConfig"]["temperature"], 1.0);
        assert_eq!(json["generationConfig"]["maxOutputTokens"], 8192);
        // None fields should be omitted
        assert!(json["generationConfig"].get("topP").is_none());
    }

    // ── Phase 2 Type Tests ──

    #[test]
    fn serialize_tool_definition() {
        let tool_def = ToolDefinition {
            function_declarations: vec![FunctionDeclaration {
                name: "read_file".into(),
                description: "Read a file".into(),
                parameters: Parameters {
                    schema_type: "object".into(),
                    properties: {
                        let mut map = serde_json::Map::new();
                        map.insert("path".into(), serde_json::json!({"type": "string", "description": "File path"}));
                        map
                    },
                    required: Some(vec!["path".into()]),
                },
            }],
        };
        let json = serde_json::to_value(&tool_def).unwrap();
        assert!(json.get("functionDeclarations").is_some());
        let decl = &json["functionDeclarations"][0];
        assert_eq!(decl["name"], "read_file");
        assert_eq!(decl["description"], "Read a file");
        assert_eq!(decl["parameters"]["type"], "object");
        assert_eq!(decl["parameters"]["properties"]["path"]["type"], "string");
        assert_eq!(decl["parameters"]["required"][0], "path");
    }

    #[test]
    fn serialize_function_declaration_with_all_param_types() {
        let params = Parameters {
            schema_type: "object".into(),
            properties: {
                let mut map = serde_json::Map::new();
                map.insert("name".into(), serde_json::json!({"type": "string", "description": "Name"}));
                map.insert("count".into(), serde_json::json!({"type": "integer", "description": "Count"}));
                map.insert("verbose".into(), serde_json::json!({"type": "boolean", "description": "Verbose"}));
                map
            },
            required: Some(vec!["name".into()]),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["properties"]["name"]["type"], "string");
        assert_eq!(json["properties"]["count"]["type"], "integer");
        assert_eq!(json["properties"]["verbose"]["type"], "boolean");
        assert_eq!(json["required"], serde_json::json!(["name"]));
    }

    #[test]
    fn serialize_parameters_omits_required_when_none() {
        let params = Parameters {
            schema_type: "object".into(),
            properties: serde_json::Map::new(),
            required: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert!(json.get("required").is_none());
    }

    #[test]
    fn serialize_tool_config() {
        let config = ToolConfig {
            function_calling_config: FunctionCallingConfig {
                mode: "AUTO".into(),
            },
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["functionCallingConfig"]["mode"], "AUTO");
    }

    #[test]
    fn serialize_request_with_tools() {
        let request = GenerateContentRequest {
            contents: vec![Content::user("test")],
            system_instruction: None,
            generation_config: None,
            tools: Some(vec![ToolDefinition {
                function_declarations: vec![FunctionDeclaration {
                    name: "test_tool".into(),
                    description: "A test tool".into(),
                    parameters: Parameters {
                        schema_type: "object".into(),
                        properties: serde_json::Map::new(),
                        required: None,
                    },
                }],
            }]),
            tool_config: Some(ToolConfig {
                function_calling_config: FunctionCallingConfig {
                    mode: "AUTO".into(),
                },
            }),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("tools").is_some());
        assert!(json.get("toolConfig").is_some());
        assert_eq!(json["tools"][0]["functionDeclarations"][0]["name"], "test_tool");
        assert_eq!(json["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    }

    #[test]
    fn content_function_responses_constructor() {
        let content = Content::function_responses(vec![Part::FunctionResponse {
            name: "read_file".into(),
            response: serde_json::json!({"content": "data"}),
        }]);
        assert_eq!(content.role.as_deref(), Some("user"));
        assert_eq!(content.parts.len(), 1);
        assert!(matches!(&content.parts[0], Part::FunctionResponse { name, .. } if name == "read_file"));
    }

    #[test]
    fn response_function_calls_extraction() {
        let response = GenerateContentResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".into()),
                    parts: vec![
                        Part::FunctionCall {
                            name: "read_file".into(),
                            args: serde_json::json!({"path": "a.rs"}),
                            thought_signature: None,
                        },
                        Part::FunctionCall {
                            name: "list_directory".into(),
                            args: serde_json::json!({}),
                            thought_signature: None,
                        },
                    ],
                }),
                finish_reason: Some("STOP".into()),
                safety_ratings: vec![],
            }],
            usage_metadata: None,
            model_version: None,
        };
        assert_eq!(response.function_calls().len(), 2);
        assert!(response.has_function_calls());
    }

    #[test]
    fn response_function_calls_empty_for_text_only() {
        let response = GenerateContentResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".into()),
                    parts: vec![Part::Text("just text".into())],
                }),
                finish_reason: Some("STOP".into()),
                safety_ratings: vec![],
            }],
            usage_metadata: None,
            model_version: None,
        };
        assert!(response.function_calls().is_empty());
        assert!(!response.has_function_calls());
    }

    #[test]
    fn response_function_calls_empty_for_no_candidates() {
        let response = GenerateContentResponse {
            candidates: vec![],
            usage_metadata: None,
            model_version: None,
        };
        assert!(response.function_calls().is_empty());
        assert!(!response.has_function_calls());
    }
}

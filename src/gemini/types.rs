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
    // tools and tool_config added in Phase 2
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

// ── Part Enum (custom deserialization) ──

#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    FunctionCall { name: String, args: Value },
    FunctionResponse { name: String, response: Value },
    InlineData { mime_type: String, data: String },
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
            Part::FunctionCall { name, args } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(
                    "functionCall",
                    &serde_json::json!({"name": name, "args": args}),
                )?;
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
        let key: String = map
            .next_key()?
            .ok_or_else(|| de::Error::custom("empty Part object"))?;

        match key.as_str() {
            "text" => {
                let text: String = map.next_value()?;
                Ok(Part::Text(text))
            }
            "functionCall" => {
                let call: FunctionCallRaw = map.next_value()?;
                Ok(Part::FunctionCall {
                    name: call.name,
                    args: call.args,
                })
            }
            "functionResponse" => {
                let resp: FunctionResponseRaw = map.next_value()?;
                Ok(Part::FunctionResponse {
                    name: resp.name,
                    response: resp.response,
                })
            }
            "inlineData" => {
                let data: InlineDataRaw = map.next_value()?;
                Ok(Part::InlineData {
                    mime_type: data.mime_type,
                    data: data.data,
                })
            }
            other => Err(de::Error::unknown_field(
                other,
                &["text", "functionCall", "functionResponse", "inlineData"],
            )),
        }
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
            Part::FunctionCall { name, args } => {
                assert_eq!(name, "read_file");
                assert_eq!(args["path"], "/tmp/test");
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
    fn deserialize_unknown_part_fails() {
        let json = r#"{"unknownField": "value"}"#;
        let result = serde_json::from_str::<Part>(json);
        assert!(result.is_err());
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
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("systemInstruction").is_none());
        assert!(json.get("generationConfig").is_none());
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
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("systemInstruction").is_some());
        assert!(json.get("generationConfig").is_some());
        assert_eq!(json["generationConfig"]["temperature"], 1.0);
        assert_eq!(json["generationConfig"]["maxOutputTokens"], 8192);
        // None fields should be omitted
        assert!(json["generationConfig"].get("topP").is_none());
    }
}

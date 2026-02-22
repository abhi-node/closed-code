use std::fmt;
use std::time::Duration;

use backon::Retryable;
use reqwest_eventsource::EventSource;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::{GenerateContentRequest, GenerateContentResponse};

pub struct GeminiClient {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl GeminiClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
        }
    }

    /// Get the API key (needed for model switching to reconstruct client).
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Get the model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    fn url(&self, method: &str) -> String {
        format!("{}/models/{}:{}", self.base_url, self.model, method)
    }

    /// Non-streaming generate (used by sub-agents in later phases).
    pub async fn generate_content(
        &self,
        request: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse> {
        let url = self.url("generateContent");
        let api_key = &self.api_key;
        let client = &self.client;

        let response = (|| async {
            let resp = client
                .post(&url)
                .header("x-goog-api-key", api_key)
                .json(request)
                .send()
                .await?;
            Ok::<_, reqwest::Error>(resp)
        })
        .retry(
            backon::ExponentialBuilder::default()
                .with_min_delay(Duration::from_millis(500))
                .with_max_times(3),
        )
        .sleep(tokio::time::sleep)
        .when(|e: &reqwest::Error| {
            e.is_timeout()
                || e.is_connect()
                || e.status()
                    .map(|s| s == 429 || s.is_server_error())
                    .unwrap_or(false)
        })
        .notify(|err: &reqwest::Error, dur: Duration| {
            tracing::warn!("Retrying after {:?}: {}", dur, err);
        })
        .await
        .map_err(ClosedCodeError::Network)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ClosedCodeError::from_status(status.as_u16(), body));
        }

        let result: GenerateContentResponse = response.json().await?;
        Ok(result)
    }

    /// Streaming generate — returns an SSE event source.
    pub fn stream_generate_content(&self, request: &GenerateContentRequest) -> EventSource {
        let request_builder = self
            .client
            .post(format!("{}?alt=sse", self.url("streamGenerateContent")))
            .header("x-goog-api-key", &self.api_key)
            .json(request);

        EventSource::new(request_builder).expect("failed to create EventSource")
    }
}

/// Parse Retry-After header from a 429 response.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Add jitter to a duration (±25%).
pub fn with_jitter(duration: Duration) -> Duration {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let jitter_factor = 0.75 + (nanos as f64 / u32::MAX as f64) * 0.5; // 0.75–1.25
    Duration::from_millis((duration.as_millis() as f64 * jitter_factor) as u64)
}

impl fmt::Debug for GeminiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiClient")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_construction() {
        let client = GeminiClient::new("test-key".into(), "gemini-3.1-pro-preview".into());
        assert_eq!(
            client.url("generateContent"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-pro-preview:generateContent"
        );
        assert_eq!(
            client.url("streamGenerateContent"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-pro-preview:streamGenerateContent"
        );
    }

    #[test]
    fn debug_redacts_api_key() {
        let client = GeminiClient::new("super-secret-key".into(), "test-model".into());
        let debug_output = format!("{:?}", client);
        assert!(!debug_output.contains("super-secret-key"));
        assert!(debug_output.contains("[REDACTED]"));
        assert!(debug_output.contains("test-model"));
    }

    #[test]
    fn api_key_getter() {
        let client = GeminiClient::new("my-key".into(), "model".into());
        assert_eq!(client.api_key(), "my-key");
    }

    #[test]
    fn model_getter() {
        let client = GeminiClient::new("key".into(), "gemini-2.0-flash".into());
        assert_eq!(client.model(), "gemini-2.0-flash");
    }

    #[test]
    fn retry_after_parsing() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "5".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(5)));
    }

    #[test]
    fn retry_after_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn retry_after_non_numeric() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "not-a-number".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn jitter_within_range() {
        let base = Duration::from_secs(10);
        let jittered = with_jitter(base);
        // Should be within 75%–125% of base
        let min = Duration::from_millis(7500);
        let max = Duration::from_millis(12500);
        assert!(jittered >= min, "jittered {:?} < min {:?}", jittered, min);
        assert!(jittered <= max, "jittered {:?} > max {:?}", jittered, max);
    }
}

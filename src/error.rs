use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // API errors
    #[error("Gemini API error (HTTP {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("Rate limited (429). Retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("Gemini API returned no candidates")]
    EmptyResponse,

    #[error("Response blocked by safety filter: {reason}")]
    SafetyBlocked { reason: String },

    // Config errors
    #[error("Missing API key. Set GEMINI_API_KEY or pass --api-key")]
    MissingApiKey,

    #[error("Invalid mode '{0}'. Expected: explore, plan, or execute")]
    InvalidMode(String),

    // Network / IO
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    // Parse errors
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SSE stream error: {0}")]
    StreamError(String),

    #[error("Failed to parse Gemini Part: {0}")]
    PartParseError(String),
}

impl ClosedCodeError {
    /// Whether this error is transient and the request can be retried.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Network(_) => true,
            Self::ApiError { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Build from an HTTP response status + body.
    pub fn from_status(status: u16, body: String) -> Self {
        match status {
            429 => Self::RateLimited {
                retry_after_ms: 1000,
            },
            _ => Self::ApiError {
                status,
                message: body,
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, ClosedCodeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_is_retryable() {
        let err = ClosedCodeError::RateLimited {
            retry_after_ms: 1000,
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn server_error_is_retryable() {
        let err = ClosedCodeError::ApiError {
            status: 500,
            message: "internal".into(),
        };
        assert!(err.is_retryable());

        let err503 = ClosedCodeError::ApiError {
            status: 503,
            message: "unavailable".into(),
        };
        assert!(err503.is_retryable());
    }

    #[test]
    fn client_error_is_not_retryable() {
        let err = ClosedCodeError::ApiError {
            status: 400,
            message: "bad request".into(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn missing_api_key_is_not_retryable() {
        let err = ClosedCodeError::MissingApiKey;
        assert!(!err.is_retryable());
    }

    #[test]
    fn from_status_429_creates_rate_limited() {
        let err = ClosedCodeError::from_status(429, "too many".into());
        assert!(matches!(err, ClosedCodeError::RateLimited { .. }));
    }

    #[test]
    fn from_status_500_creates_api_error() {
        let err = ClosedCodeError::from_status(500, "server error".into());
        assert!(matches!(
            err,
            ClosedCodeError::ApiError {
                status: 500,
                ..
            }
        ));
    }

    #[test]
    fn from_status_400_creates_api_error() {
        let err = ClosedCodeError::from_status(400, "bad request".into());
        assert!(matches!(
            err,
            ClosedCodeError::ApiError {
                status: 400,
                ..
            }
        ));
    }

    #[test]
    fn error_display_messages() {
        let err = ClosedCodeError::MissingApiKey;
        assert_eq!(
            err.to_string(),
            "Missing API key. Set GEMINI_API_KEY or pass --api-key"
        );

        let err = ClosedCodeError::InvalidMode("bad".into());
        assert_eq!(
            err.to_string(),
            "Invalid mode 'bad'. Expected: explore, plan, or execute"
        );
    }
}

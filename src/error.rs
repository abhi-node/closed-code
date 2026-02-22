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

    // Tool errors (Phase 2)
    #[error("Tool '{name}' not found in registry")]
    ToolNotFound { name: String },

    #[error("Tool '{name}' execution failed: {message}")]
    ToolError { name: String, message: String },

    #[error("Tool-call loop exceeded max iterations ({max})")]
    ToolLoopMaxIterations { max: usize },

    #[error("Command '{command}' is not in the allowlist. Allowed: {allowed}")]
    ShellNotAllowed { command: String, allowed: String },

    #[error("Shell command failed: {0}")]
    ShellError(String),

    #[error("Shell command timed out after {seconds}s")]
    ShellTimeout { seconds: u64 },

    #[error("File too large ({size_bytes} bytes, max {max_bytes}): {path}")]
    FileTooLarge {
        path: String,
        size_bytes: u64,
        max_bytes: u64,
    },

    #[error("Binary file detected: {path}")]
    BinaryFile { path: String },

    #[error("Invalid glob pattern: {0}")]
    GlobError(String),

    #[error("Invalid regex pattern: {0}")]
    RegexError(String),

    // Agent errors (Phase 3)
    #[error("Agent '{agent_id}' failed: {message}")]
    AgentError { agent_id: String, message: String },

    #[error("Agent '{agent_id}' timed out after {seconds}s")]
    AgentTimeout { agent_id: String, seconds: u64 },

    #[error("Orchestrator exceeded max iterations ({max}) for this turn")]
    OrchestratorMaxIterations { max: usize },

    #[error("Sub-agent tool loop exceeded max iterations ({max}) for agent '{agent_id}'")]
    SubAgentMaxIterations { agent_id: String, max: usize },

    // File modification errors (Phase 4)
    #[error("Cannot modify protected path: {path}")]
    ProtectedPath { path: String },

    #[error("Approval prompt failed: {0}")]
    ApprovalError(String),

    // Configuration errors (Phase 5)
    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Invalid approval policy '{0}'. Expected: suggest, auto_edit, full_auto")]
    InvalidApprovalPolicy(String),

    #[error("Invalid personality '{0}'. Expected: friendly, pragmatic, none")]
    InvalidPersonality(String),
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

    #[test]
    fn tool_errors_are_not_retryable() {
        let errors: Vec<ClosedCodeError> = vec![
            ClosedCodeError::ToolNotFound { name: "x".into() },
            ClosedCodeError::ToolError { name: "x".into(), message: "fail".into() },
            ClosedCodeError::ToolLoopMaxIterations { max: 10 },
            ClosedCodeError::ShellNotAllowed { command: "rm".into(), allowed: "ls".into() },
            ClosedCodeError::ShellError("fail".into()),
            ClosedCodeError::ShellTimeout { seconds: 30 },
            ClosedCodeError::FileTooLarge { path: "big".into(), size_bytes: 200000, max_bytes: 100000 },
            ClosedCodeError::BinaryFile { path: "a.out".into() },
            ClosedCodeError::GlobError("bad".into()),
            ClosedCodeError::RegexError("bad".into()),
        ];
        for err in &errors {
            assert!(!err.is_retryable(), "Expected not retryable: {err}");
        }
    }

    #[test]
    fn agent_errors_are_not_retryable() {
        let errors: Vec<ClosedCodeError> = vec![
            ClosedCodeError::AgentError { agent_id: "explorer".into(), message: "fail".into() },
            ClosedCodeError::AgentTimeout { agent_id: "explorer".into(), seconds: 120 },
            ClosedCodeError::OrchestratorMaxIterations { max: 30 },
            ClosedCodeError::SubAgentMaxIterations { agent_id: "planner".into(), max: 15 },
        ];
        for err in &errors {
            assert!(!err.is_retryable(), "Expected not retryable: {err}");
        }
    }

    #[test]
    fn phase4_errors_are_not_retryable() {
        let errors: Vec<ClosedCodeError> = vec![
            ClosedCodeError::ProtectedPath { path: ".git/config".into() },
            ClosedCodeError::ApprovalError("prompt failed".into()),
        ];
        for err in &errors {
            assert!(!err.is_retryable(), "Expected not retryable: {err}");
        }
    }

    #[test]
    fn phase5_errors_are_not_retryable() {
        let errors: Vec<ClosedCodeError> = vec![
            ClosedCodeError::ConfigError("bad toml".into()),
            ClosedCodeError::InvalidApprovalPolicy("bad".into()),
            ClosedCodeError::InvalidPersonality("bad".into()),
        ];
        for err in &errors {
            assert!(!err.is_retryable(), "Expected not retryable: {err}");
        }
    }

    #[test]
    fn phase5_error_display_messages() {
        assert_eq!(
            ClosedCodeError::ConfigError("bad toml".into()).to_string(),
            "Configuration error: bad toml"
        );
        assert_eq!(
            ClosedCodeError::InvalidApprovalPolicy("bad".into()).to_string(),
            "Invalid approval policy 'bad'. Expected: suggest, auto_edit, full_auto"
        );
        assert_eq!(
            ClosedCodeError::InvalidPersonality("bad".into()).to_string(),
            "Invalid personality 'bad'. Expected: friendly, pragmatic, none"
        );
    }

    #[test]
    fn phase4_error_display_messages() {
        assert_eq!(
            ClosedCodeError::ProtectedPath { path: ".git/config".into() }.to_string(),
            "Cannot modify protected path: .git/config"
        );
        assert_eq!(
            ClosedCodeError::ApprovalError("prompt failed".into()).to_string(),
            "Approval prompt failed: prompt failed"
        );
    }

    #[test]
    fn tool_error_display_messages() {
        assert_eq!(
            ClosedCodeError::ToolNotFound { name: "read_file".into() }.to_string(),
            "Tool 'read_file' not found in registry"
        );
        assert_eq!(
            ClosedCodeError::ToolError { name: "grep".into(), message: "bad regex".into() }.to_string(),
            "Tool 'grep' execution failed: bad regex"
        );
        assert_eq!(
            ClosedCodeError::ToolLoopMaxIterations { max: 10 }.to_string(),
            "Tool-call loop exceeded max iterations (10)"
        );
        assert_eq!(
            ClosedCodeError::ShellNotAllowed { command: "rm".into(), allowed: "ls, cat".into() }.to_string(),
            "Command 'rm' is not in the allowlist. Allowed: ls, cat"
        );
        assert_eq!(
            ClosedCodeError::ShellError("failed".into()).to_string(),
            "Shell command failed: failed"
        );
        assert_eq!(
            ClosedCodeError::ShellTimeout { seconds: 30 }.to_string(),
            "Shell command timed out after 30s"
        );
        assert_eq!(
            ClosedCodeError::FileTooLarge { path: "big.bin".into(), size_bytes: 200000, max_bytes: 100000 }.to_string(),
            "File too large (200000 bytes, max 100000): big.bin"
        );
        assert_eq!(
            ClosedCodeError::BinaryFile { path: "a.out".into() }.to_string(),
            "Binary file detected: a.out"
        );
        assert_eq!(
            ClosedCodeError::GlobError("invalid [".into()).to_string(),
            "Invalid glob pattern: invalid ["
        );
        assert_eq!(
            ClosedCodeError::RegexError("bad regex".into()).to_string(),
            "Invalid regex pattern: bad regex"
        );
    }
}

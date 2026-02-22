use crate::gemini::types::UsageMetadata;

/// Cumulative token usage tracker for a session.
#[derive(Debug, Default, Clone)]
pub struct SessionUsage {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub api_calls: u64,
}

impl SessionUsage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulate usage from a single API response.
    pub fn accumulate(&mut self, usage: &UsageMetadata) {
        self.total_prompt_tokens += usage.prompt_token_count.unwrap_or(0) as u64;
        self.total_completion_tokens += usage.candidates_token_count.unwrap_or(0) as u64;
        self.total_tokens += usage.total_token_count.unwrap_or(0) as u64;
        self.api_calls += 1;
    }
}

impl std::fmt::Display for SessionUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} prompt + {} completion = {} total ({} API calls)",
            format_number(self.total_prompt_tokens),
            format_number(self.total_completion_tokens),
            format_number(self.total_tokens),
            self.api_calls,
        )
    }
}

/// Format a number with comma separators: 1234567 → "1,234,567"
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_usage_default() {
        let usage = SessionUsage::new();
        assert_eq!(usage.total_prompt_tokens, 0);
        assert_eq!(usage.total_completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.api_calls, 0);
    }

    #[test]
    fn session_usage_accumulate() {
        let mut usage = SessionUsage::new();
        let meta = UsageMetadata {
            prompt_token_count: Some(100),
            candidates_token_count: Some(50),
            total_token_count: Some(150),
        };
        usage.accumulate(&meta);
        assert_eq!(usage.total_prompt_tokens, 100);
        assert_eq!(usage.total_completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
        assert_eq!(usage.api_calls, 1);
    }

    #[test]
    fn session_usage_multiple_accumulate() {
        let mut usage = SessionUsage::new();
        let meta1 = UsageMetadata {
            prompt_token_count: Some(100),
            candidates_token_count: Some(50),
            total_token_count: Some(150),
        };
        let meta2 = UsageMetadata {
            prompt_token_count: Some(200),
            candidates_token_count: Some(75),
            total_token_count: Some(275),
        };
        usage.accumulate(&meta1);
        usage.accumulate(&meta2);
        assert_eq!(usage.total_prompt_tokens, 300);
        assert_eq!(usage.total_completion_tokens, 125);
        assert_eq!(usage.total_tokens, 425);
        assert_eq!(usage.api_calls, 2);
    }

    #[test]
    fn session_usage_accumulate_with_none_fields() {
        let mut usage = SessionUsage::new();
        let meta = UsageMetadata {
            prompt_token_count: None,
            candidates_token_count: Some(50),
            total_token_count: None,
        };
        usage.accumulate(&meta);
        assert_eq!(usage.total_prompt_tokens, 0);
        assert_eq!(usage.total_completion_tokens, 50);
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.api_calls, 1);
    }

    #[test]
    fn session_usage_display() {
        let mut usage = SessionUsage::new();
        let meta = UsageMetadata {
            prompt_token_count: Some(1234),
            candidates_token_count: Some(567),
            total_token_count: Some(1801),
        };
        usage.accumulate(&meta);
        let display = usage.to_string();
        assert!(display.contains("1,234 prompt"));
        assert!(display.contains("567 completion"));
        assert!(display.contains("1,801 total"));
        assert!(display.contains("1 API calls"));
    }

    #[test]
    fn format_number_basic() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(42), "42");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }
}

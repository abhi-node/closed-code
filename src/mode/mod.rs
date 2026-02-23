use std::fmt;
use std::str::FromStr;

use crate::error::ClosedCodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Explore,
    Plan,
    Execute,
    Auto,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mode::Explore => write!(f, "explore"),
            Mode::Plan => write!(f, "plan"),
            Mode::Execute => write!(f, "execute"),
            Mode::Auto => write!(f, "auto"),
        }
    }
}

impl FromStr for Mode {
    type Err = ClosedCodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "explore" => Ok(Mode::Explore),
            "plan" => Ok(Mode::Plan),
            "execute" => Ok(Mode::Execute),
            "auto" => Ok(Mode::Auto),
            other => Err(ClosedCodeError::InvalidMode(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_modes() {
        assert_eq!("explore".parse::<Mode>().unwrap(), Mode::Explore);
        assert_eq!("plan".parse::<Mode>().unwrap(), Mode::Plan);
        assert_eq!("execute".parse::<Mode>().unwrap(), Mode::Execute);
        assert_eq!("auto".parse::<Mode>().unwrap(), Mode::Auto);
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!("EXPLORE".parse::<Mode>().unwrap(), Mode::Explore);
        assert_eq!("Plan".parse::<Mode>().unwrap(), Mode::Plan);
        assert_eq!("EXECUTE".parse::<Mode>().unwrap(), Mode::Execute);
        assert_eq!("AUTO".parse::<Mode>().unwrap(), Mode::Auto);
    }

    #[test]
    fn parse_invalid_mode() {
        let result = "invalid".parse::<Mode>();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::InvalidMode(s) if s == "invalid"
        ));
    }

    #[test]
    fn display_roundtrip() {
        for mode in [Mode::Explore, Mode::Plan, Mode::Execute, Mode::Auto] {
            let s = mode.to_string();
            let parsed: Mode = s.parse().unwrap();
            assert_eq!(mode, parsed);
        }
    }
}

//! Configuration failures.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    /// Names the offending variable so the misconfig is obvious at boot.
    #[error("invalid value for {var}: {message}")]
    Parse { var: String, message: String },
    #[error("configuration validation failed: {0}")]
    Validation(#[from] validator::ValidationErrors),
}

impl ConfigError {
    pub fn parse(var: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Parse {
            var: var.into(),
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, ConfigError>;

//! Configuration failures.

use thiserror::Error;

/// A configuration load failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Names the offending variable so the misconfig is obvious at boot.
    #[error("invalid value for {var}: {message}")]
    Parse {
        /// The offending `NESTRS_<NS>__<KEY>` variable name.
        var: String,
        /// Why the value was rejected.
        message: String,
    },
    /// A loaded config failed `validator::Validate`.
    #[error("configuration validation failed: {0}")]
    Validation(#[from] validator::ValidationErrors),
}

impl ConfigError {
    /// Build a [`Parse`](Self::Parse) error naming the variable and the reason.
    pub fn parse(var: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Parse {
            var: var.into(),
            message: message.into(),
        }
    }
}

/// A `Result` whose error is a [`ConfigError`].
pub type Result<T> = std::result::Result<T, ConfigError>;

//! Configuration failures.

use thiserror::Error;

/// A configuration failure: a load/parse error from figment, or a violation of a
/// config type's declarative `validator` rules ([`load_validated`](crate::load_validated)).
// `figment::Error` is ~208 bytes; boxing it keeps every `Result<_, ConfigError>`
// small, satisfying `clippy::result_large_err` without bloating the hot path.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration error: {0}")]
    Source(Box<figment::Error>),
    #[error("configuration validation failed: {0}")]
    Validation(#[from] validator::ValidationErrors),
}

impl From<figment::Error> for ConfigError {
    fn from(value: figment::Error) -> Self {
        Self::Source(Box::new(value))
    }
}

pub type Result<T> = std::result::Result<T, ConfigError>;

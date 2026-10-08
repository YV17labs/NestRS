//! `#[config]` — its `Validate` derive must resolve through the framework's
//! `crate = ` override, never `::validator::` at the call site.

use nest_rs::config::config;

/// Minimal config.
#[config(namespace = "macro_hygiene")]
#[derive(Clone, Debug)]
pub struct MacroHygieneConfig {
    /// A validation rule, so the emitted `Validate` derive is exercised.
    #[validate(range(min = 1))]
    pub retries: u32,
}

impl Default for MacroHygieneConfig {
    fn default() -> Self {
        Self { retries: 1 }
    }
}

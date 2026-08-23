//! `#[config]` — a namespaced settings struct resolved from the `.env` cascade.
//!
//! The decorator carries `Validate` and points it back at the framework's own
//! copy through a `crate = ` override; without it the derive would emit
//! `::validator::` against *this* crate's prelude, which holds nothing. That is
//! exactly the failure this crate exists to catch.

use nest_rs::config::config;

/// Minimal config. Every field is settable from `NESTRS_MACRO_HYGIENE__*` and from a
/// pinned base, per the dual-path rule.
#[config(namespace = "macro_hygiene")]
#[derive(Clone, Debug)]
pub struct HygieneConfig {
    /// A field with a validation rule, so the emitted `Validate` derive is
    /// actually exercised rather than merely present.
    #[validate(range(min = 1))]
    pub retries: u32,
}

impl Default for HygieneConfig {
    fn default() -> Self {
        Self { retries: 1 }
    }
}

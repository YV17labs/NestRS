//! One canary per `clippy.toml` entry.
//!
//! A path in `clippy.toml` that stops resolving — a typo, an item moved
//! upstream — is only a warning, so the rule it held switches itself off and
//! `-D warnings` stays green. Each canary uses the item its entry forbids under
//! an `#[expect]`: a dead entry leaves the expectation unfulfilled, which is an
//! error. An entry is added with its canary, at the end of its list here.

/// `disallowed-methods`: `std::env::var`.
#[expect(
    clippy::disallowed_methods,
    reason = "canary: proves clippy.toml's std::env::var entry resolves"
)]
pub fn env_var() -> bool {
    std::env::var("PATH").is_ok()
}

/// `disallowed-methods`: `std::env::var_os`.
#[expect(
    clippy::disallowed_methods,
    reason = "canary: proves clippy.toml's std::env::var_os entry resolves"
)]
pub fn env_var_os() -> bool {
    std::env::var_os("PATH").is_some()
}

/// `disallowed-macros`: `tokio::main`. Behind `cfg(test)` because the witness
/// declares no `tokio`; the unit-test build reaches it as a dev-dependency.
///
/// Its expectation is the crate's (`lib.rs`), not this item's: clippy reports
/// an attribute macro against the crate root, outside the item it decorates,
/// so no narrower `#[expect]` can hold it.
#[cfg(test)]
#[expect(dead_code, reason = "a canary is compiled, never called")]
#[tokio::main]
async fn tokio_main() {}

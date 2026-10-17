//! Integration tests for `nest-rs-pipes`, mirroring `src/`: each pipe through
//! `Pipe::transform`, the call every transport's binding makes, in process.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod pipe;
mod pipes;

use nest_rs_pipes::PipeError;

/// A live secret, sent where a pipe expects something else.
const SECRET: &str = "sk_live_51HsecretTOKEN";

/// What an edge forwards of a refusal: its message, and the details it renders
/// as `errors`.
fn carried(refusal: &PipeError) -> String {
    let details = refusal
        .details()
        .map(ToString::to_string)
        .unwrap_or_default();
    format!("{} {details}", refusal.message())
}

/// The first eight-character run of `secret` that `text` spells, so a refusal
/// quoting it cut short or elided still counts as quoting it.
fn quoted_run<'s>(text: &str, secret: &'s str) -> Option<&'s str> {
    (0..=secret.len().saturating_sub(8))
        .filter_map(|start| secret.get(start..start + 8))
        .find(|run| text.contains(run))
}

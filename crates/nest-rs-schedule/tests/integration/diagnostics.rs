//! trybuild snapshots of the `#[scheduled]` compile diagnostics.
//!
//! The impl half of an `on_provider` pair has one wrong shape — the provider
//! struct — and the rule is that it names the sibling instead of reporting
//! syn's `expected impl`. Pinned here so the wording cannot drift back.
//!
//! `version = "…"` is pinned for the same reason in reverse: the argument is
//! refused rather than ignored, and the sentence has to name what a clock-driven
//! transport does instead of what it does not have.

#[test]
fn scheduled_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

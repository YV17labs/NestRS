//! trybuild snapshots of the `#[listeners]` compile diagnostics.
//!
//! The impl half of an `on_provider` pair has one wrong shape — the provider
//! struct — and the rule is that it names the sibling instead of reporting
//! syn's `expected impl`. Pinned here so the wording cannot drift back.
//!
//! `version = "…"` is pinned beside it: an in-process listener has no address,
//! so the refusal names payload evolution rather than a spelling this decorator
//! would accept.

#[test]
fn listeners_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

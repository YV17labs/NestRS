//! trybuild snapshot of the `#[scheduled]` wrong-shape diagnostic.
//!
//! The impl half of an `on_provider` pair has one wrong shape — the provider
//! struct — and the rule is that it names the sibling instead of reporting
//! syn's `expected impl`. Pinned here so the wording cannot drift back.

#[test]
fn scheduled_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

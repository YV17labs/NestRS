//! trybuild snapshots of the `#[gateway]` / `#[messages]` compile diagnostics.
//!
//! Four rules are pinned here because each one is a security or DX contract the
//! exact wording is part of: an HTTP-only layer attribute is rejected with a
//! redirect to its real home rather than becoming a silent no-op; a message with
//! **no declared posture** does not compile (`CLAUDE.md`'s *no authn/authz
//! decision outside a guard* — silence is not a posture); the two wrong-shape
//! cases pin the *one decorator, one item shape* rule, each naming the sibling
//! that belongs on the shape the developer reached for; and an unknown
//! `#[gateway]` key names every accepted one, which is what keeps that sentence
//! honest as the grammar grows.

#[test]
fn gateway_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

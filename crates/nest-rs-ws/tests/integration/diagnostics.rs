//! trybuild snapshots of the `#[gateway]` compile diagnostics — an HTTP-only
//! layer attribute on a gateway is rejected with a redirect to its real home,
//! never a silent no-op; the exact wording is pinned here.

#[test]
fn gateway_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

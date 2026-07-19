//! trybuild snapshots of the `#[resolver]` compile diagnostics — the
//! mandatory-posture check is security-load-bearing (an operation with no
//! declared posture must not compile), so its exact wording is pinned here.

#[test]
fn resolver_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

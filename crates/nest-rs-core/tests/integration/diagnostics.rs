//! trybuild snapshots of the core macro compile diagnostics — the exact error
//! a developer sees is part of the framework's contract (CORE-I10), mirroring
//! the http/graphql/ws macros' own suites. Boot-time diagnostics (missing
//! dependency, unimported module) are runtime errors pinned by `access.rs`;
//! this covers the compile-time ones.

#[test]
fn core_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

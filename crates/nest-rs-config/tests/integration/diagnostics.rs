//! trybuild snapshots of the `#[config]` macro compile diagnostics — the exact
//! error a developer sees is part of the framework's contract (CORE-I10),
//! mirroring the core/http/graphql/ws macros' own suites.

#[test]
fn config_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

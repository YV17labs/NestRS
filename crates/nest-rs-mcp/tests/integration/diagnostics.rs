//! trybuild snapshots of the `#[mcp]` / `#[tools]` compile diagnostics.
//!
//! Two of them pin the *one decorator, one item shape* rule (`CLAUDE.md`): a
//! decorator on the wrong shape names its sibling, because the shape the
//! developer reached for does exist — it is spelled with the other decorator,
//! and a macro that merely said "expected struct" would send them looking for a
//! bug in their own code.

#[test]
fn mcp_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

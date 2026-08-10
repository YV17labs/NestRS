//! trybuild snapshots of the `#[resolver]` / `#[operations]` compile
//! diagnostics — the mandatory-posture check is security-load-bearing (an
//! operation with no declared posture must not compile), so its exact wording is
//! pinned here. The two wrong-shape cases pin the *one decorator, one item
//! shape* rule (`CLAUDE.md`): each decorator names the sibling that belongs on
//! the shape the developer reached for.

#[test]
fn resolver_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

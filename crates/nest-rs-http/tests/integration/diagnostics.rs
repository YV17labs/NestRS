//! trybuild snapshots of the `#[crud]` compile diagnostics — the exact error a
//! developer sees is part of the framework's contract, so a wording or span
//! regression fails this test instead of shipping silently. Boot-time
//! diagnostics (missing dependency, unimported module) are runtime errors,
//! pinned by `nest-rs-core`'s integration tests — this suite covers the
//! compile-time ones.

#[test]
fn crud_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

//! trybuild snapshots of the `#[controller]` / `#[routes]` / `#[crud]` compile
//! diagnostics — the
//! exact error a developer sees is part of the framework's contract, so a
//! wording or span regression fails this test instead of shipping silently. Two
//! of them pin the *one decorator, one item shape* rule (`CLAUDE.md`): each half
//! on the wrong shape names its sibling, because the shape the developer reached
//! for does exist — it is spelled with the other decorator, and a macro that
//! merely said "expected struct" would send them hunting a bug in their own
//! code. Boot-time
//! diagnostics (missing dependency, unimported module) are runtime errors,
//! pinned by `nest-rs-core`'s integration tests — this suite covers the
//! compile-time ones.

#[test]
fn http_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

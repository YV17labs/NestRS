//! trybuild snapshots of the `#[mcp]` / `#[tools]` compile diagnostics.
//!
//! Two of them pin the *one decorator, one item shape* rule (`CLAUDE.md`): a
//! decorator on the wrong shape names its sibling, because the shape the
//! developer reached for does exist — it is spelled with the other decorator,
//! and a macro that merely said "expected struct" would send them looking for a
//! bug in their own code.
//!
//! The rest pin the argument grammar itself, and the split between them is the
//! point: a key that belongs to *something* names its owner — `version` with
//! this edge's own answer (the address is the whole path, the server's version
//! is the app's one declaration), every other field of the server's identity
//! with the seam that declares it — while a key that is nobody's gets the list
//! of what remains. A bare "unknown key" for the first kind is the silence
//! `CLAUDE.md` counts as a defect.

#[test]
fn mcp_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

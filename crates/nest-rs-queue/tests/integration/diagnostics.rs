//! trybuild snapshots of the `#[processor]` / `#[process]` compile diagnostics —
//! `concurrency` was a real key, so its removal has to read as a decision with a
//! replacement ("run more replicas"), not as a typo in an unknown-key list.
//! `version` never was one, and gets the same treatment for the same reason: a
//! queue is addressed by its name, and the developer arriving from
//! `#[controller(version = "1")]` is owed that answer rather than silence.

#[test]
fn process_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

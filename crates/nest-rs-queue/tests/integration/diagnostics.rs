//! trybuild snapshots of the `#[process]` compile diagnostics — `concurrency`
//! was a real key, so its removal has to read as a decision with a replacement
//! ("run more replicas"), not as a typo in an unknown-key list.

#[test]
fn process_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

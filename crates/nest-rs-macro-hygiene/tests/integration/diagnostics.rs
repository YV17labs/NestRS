//! trybuild snapshots of every decorator's refusals, one fixture folder per
//! umbrella module, written through `nest_rs::`.
//!
//! One crate, one `TestCases`: ring's build script tracks the crate under test
//! (cargo#16134), so suites spread over crates rebuild ring for each other.

#[test]
fn every_decorator_refuses_in_the_words_pinned() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/config/*.rs");
    t.compile_fail("tests/integration/diagnostics/core/*.rs");
    t.compile_fail("tests/integration/diagnostics/events/*.rs");
    t.compile_fail("tests/integration/diagnostics/graphql/*.rs");
    t.compile_fail("tests/integration/diagnostics/health/*.rs");
    t.compile_fail("tests/integration/diagnostics/http/*.rs");
    t.compile_fail("tests/integration/diagnostics/mcp/*.rs");
    t.compile_fail("tests/integration/diagnostics/queue/*.rs");
    // `#[expose(…, graphql)]` without the `graphql` feature is not pinned: every
    // build that runs this suite has the feature on.
    t.compile_fail("tests/integration/diagnostics/resource/*.rs");
    t.compile_fail("tests/integration/diagnostics/schedule/*.rs");
    t.compile_fail("tests/integration/diagnostics/seaorm/*.rs");
    t.compile_fail("tests/integration/diagnostics/ws/*.rs");
}

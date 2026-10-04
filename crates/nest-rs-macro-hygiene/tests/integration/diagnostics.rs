//! trybuild snapshots of every decorator's refusals, one folder of fixtures per
//! umbrella module, each written through `nest_rs::` as a developer writes it:
//! the exact error a developer reads is part of the contract (CORE-I10), so a
//! wording or span regression fails here instead of shipping.
//!
//! They share this one crate on purpose. trybuild runs cargo with the identity
//! of the crate under test, and ring's build script tracks it (cargo#16134), so
//! suites spread over several crates rebuilt ring and its dependents for each
//! other in the `target/tests/trybuild` they share. One `TestCases` compiles
//! every fixture in one cargo run.
//!
//! Boot-time diagnostics (a missing dependency, an unimported module) are
//! runtime errors, pinned by `nest-rs-core`'s suite.

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

//! trybuild snapshots of every decorator's refusals, one fixture folder per
//! umbrella module, written through `nest_rs::`.
//!
//! One crate, one `TestCases`: ring's build script tracks the crate under test
//! (cargo#16134), so suites spread over crates rebuild ring for each other.
//!
//! A refusal that lists a trait's implementors lists what the build links, so
//! each test pins its snapshots to one feature set: the crate's default, and
//! `graphql` alone for what a build without `authz` and `seaorm` says.

#[cfg(all(
    feature = "config",
    feature = "events",
    feature = "graphql",
    feature = "health",
    feature = "http",
    feature = "mcp",
    feature = "queue",
    feature = "schedule",
    feature = "seaorm",
    feature = "ws",
    not(feature = "redis-queue"),
    not(feature = "redis-schedule"),
    not(feature = "redis-throttler"),
    not(feature = "testing"),
))]
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
    t.compile_fail("tests/integration/diagnostics/schedule/*.rs");
    // `#[expose(…, graphql)]` without the `graphql` feature is not pinned: every
    // build that runs this suite has the feature on.
    t.compile_fail("tests/integration/diagnostics/seaorm/*.rs");
    t.compile_fail("tests/integration/diagnostics/ws/*.rs");
}

/// What an attribute says when its expansion needs a feature the build lacks:
/// run by `just test` as `--no-default-features --features graphql`.
#[cfg(all(
    feature = "graphql",
    not(feature = "http"),
    not(feature = "seaorm"),
    not(feature = "ws"),
    not(feature = "mcp"),
))]
#[test]
fn an_attribute_names_the_feature_its_expansion_needs() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/graphql/alone/*.rs");
}

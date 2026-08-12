//! trybuild snapshots of the `#[expose]` / `#[wire_enum]` compile diagnostics.
//!
//! The exposure decorators refuse a lot: a misplaced `via`, a `#[wire_default]`
//! on a column the wire already carries, a relation with no service to hang its
//! loader on. Each refusal is a sentence a developer reads instead of a
//! backtrace through a hundred lines of expansion, so the wording and the span
//! are part of the contract and a regression fails here rather than shipping.
//!
//! Every fixture's `#[expose]` **drops the item** it decorated — the decorator
//! returns the error alone — so a snapshot holds exactly one error and no
//! cascade from a `Model` that never came out the other side. That is why the
//! fixtures carry no `Relation` enum or `ActiveModelBehavior` impl: with the
//! entity gone, the ORM derives never run.
//!
//! Two diagnostics are deliberately absent, and they are the same one twice:
//! `#[expose(…, graphql)]` and `#[wire_enum(graphql)]` each refuse the flag when
//! the crate's `graphql` feature is off, under `#[cfg(not(feature =
//! "graphql"))]` in the macro crate. This suite's own dev-dependency on
//! `nest-rs-authz` (`features = ["graphql"]`) turns that feature on for the test
//! target — deliberately, so the GraphQL fixtures next door compile under a
//! plain `cargo nextest run --workspace` — and trybuild builds its project from
//! those same dev-dependencies. The refusal therefore does not exist in any
//! build that can run this suite; pinning it would mean a third suite name, and
//! the layout allows two.

#[test]
fn exposure_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

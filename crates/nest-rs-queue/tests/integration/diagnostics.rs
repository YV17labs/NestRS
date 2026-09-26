//! trybuild snapshots of the `#[queue]`, `#[processor]` and `#[process]` compile
//! diagnostics — **one per refusing site**, so a refusal that stops naming its
//! fact, or stops firing at all, fails here rather than at a developer's desk.
//!
//! A site is a wrong value, a wrong key, a wrong item shape or a wrong
//! signature, and `throttle(..)` counts as its own grammar: it holds a key set
//! and owes the same three refusals the outer one does, which the `grammars`
//! join cannot see because it derives its members from string-literal arguments
//! and this one's are built with `format!`. Covered here by hand for that
//! reason.
//!
//! One snapshot is not a refusal the macro words but the **bound it emits**:
//! `process_job_type_is_the_queues` pins `__nestrs_assert_queue_job`, the
//! decorator's headline promise, which nothing exercised — the whole assertion
//! could be deleted with every test still green.
//!
//! `version` is pinned too although it never was a key: a queue is addressed by
//! its name, and the developer arriving from `#[controller(version = "1")]` is
//! owed that answer rather than silence.

#[test]
fn process_macro_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

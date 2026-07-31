//! trybuild snapshot of the by-id binding diagnostic.
//!
//! `Bind<A, S>` / `bind::<A, S>` take the **action first, the service second**;
//! 1.1.x had them the other way round. Swapping them trips two trait bounds at
//! once, and rustc reports both against the `#[crud]` attribute on the impl
//! block rather than the offending parameter — so the `on_unimplemented` notes
//! on `ActionMarker` and `CrudService` are what actually name the mistake. The
//! wording is part of the upgrade contract, so a regression fails here.

#[test]
fn bind_parameter_order_diagnostics() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

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
    // ring's build script tracks both, and cargo compares them against what trybuild's cargo inherits
    // (cargo#16134): left set, this suite and nest-rs-resource's rebuild ring and its dependents for each other.
    // SAFETY: nextest runs this test alone in its process, and nothing else reads the environment yet.
    #[expect(
        unsafe_code,
        reason = "trybuild's cargo must not inherit this package's identity"
    )]
    unsafe {
        std::env::remove_var("CARGO_MANIFEST_DIR");
        std::env::remove_var("CARGO_PKG_NAME");
    }
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/integration/diagnostics/*.rs");
}

//! The `#[config]` decorator, re-exported by `nest-rs-config`.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod config;

/// Mark a struct as a namespaced configuration.
///
///
/// ```ignore
/// #[config(namespace = "seaorm")]
/// #[derive(Clone, Debug, serde::Deserialize)]
/// pub struct SeaOrmConfig {
///     pub url: String,
///     #[validate(range(min = 1))]
///     pub max_connections: u32,
/// }
/// ```
///
/// # Expands to
///
/// ```ignore
/// impl ::nest_rs_config::Namespaced for SeaOrmConfig {
///     const NAMESPACE: &'static str = "seaorm";
///     const DECLARATION: &'static str = concat!(module_path!(), "::", "SeaOrmConfig");
/// }
///
/// ::nest_rs_config::inventory::submit! {
///     ::nest_rs_config::ConfigNamespace::new("seaorm", concat!(module_path!(), "::", "SeaOrmConfig"))
/// }
/// ```
///
/// The submission files the namespace with the link-time registry the
/// unclaimed-variable report reads (`nest_rs_config::unclaimed`), so a variable
/// spelling this namespace with other separators is reported at boot — and a
/// second struct declaring the same namespace is refused at the read of either.
///
/// Must sit **above** the derives so it sees them intact. `namespace` must be
/// a non-empty lowercase string.
/// Carries the `Validate` derive itself, pointed back at the framework's own
/// copy, so a `#[config]` struct declares no `validator` and keeps no version
/// aligned. `#[config(namespace = "…", validate = "manual")]` suppresses it for
/// a config that validates across fields and writes `impl Validate` by hand.
#[proc_macro_attribute]
pub fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(config::config(args, input).into()).into()
}

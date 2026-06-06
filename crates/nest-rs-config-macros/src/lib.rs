//! The `#[config]` decorator, re-exported by `nestrs-config`.

use proc_macro::TokenStream;

mod config;

/// Mark a struct as a namespaced configuration.
///
/// The struct must derive `serde::Deserialize` and `validator::Validate`. The
/// macro emits `impl ::nest_rs_config::Config` with `NAMESPACE = <arg>`.
///
/// ```ignore
/// #[config(namespace = "database")]
/// #[derive(Clone, Debug, serde::Deserialize, validator::Validate)]
/// pub struct DatabaseConfig {
///     pub url: String,
///     #[validate(range(min = 1))]
///     pub max_connections: u32,
/// }
/// ```
///
/// Must sit **above** the derives so it sees them intact. `namespace` must be
/// a non-empty lowercase string.
#[proc_macro_attribute]
pub fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    config::config(args, input)
}

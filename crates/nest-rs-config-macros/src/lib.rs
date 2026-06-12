//! The `#[config]` decorator, re-exported by `nestrs-config`.

use proc_macro::TokenStream;

mod config;

/// Mark a struct as a namespaced configuration.
///
/// The struct must derive `serde::Deserialize` and `validator::Validate`. The
/// macro emits **only** `impl ::nest_rs_config::Namespaced` (the `const
/// NAMESPACE = <arg>`); you write `impl Config` (`from_env`) yourself — the
/// namespace const is the single shared piece, so the dual-path config rule
/// (env `NESTRS_<NAMESPACE>__*` *and* the pinned struct) stays the user's.
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
/// # Expands to
///
/// ```ignore
/// // the struct, unchanged, plus:
/// impl ::nest_rs_config::Namespaced for DatabaseConfig {
///     const NAMESPACE: &'static str = "database";
/// }
/// ```
///
/// Must sit **above** the derives so it sees them intact. `namespace` must be
/// a non-empty lowercase string.
#[proc_macro_attribute]
pub fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    config::config(args, input)
}

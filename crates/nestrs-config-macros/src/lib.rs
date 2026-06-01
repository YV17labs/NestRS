//! The `#[config]` decorator, re-exported by `nestrs-config`. The generated code
//! uses absolute paths (`::nestrs_config::*`), so this crate does not depend on
//! its surface crate — they resolve at the call site. The implementation lives in
//! `config`; this is the language-required proc-macro entry.

use proc_macro::TokenStream;

mod config;

/// Mark a struct as a **namespaced configuration** — the `registerAs('ns', …)`
/// analog. The struct must already derive `serde::Deserialize` and
/// `validator::Validate` (the house style: configs are explicit like entities and
/// DTOs); the macro emits an `impl ::nestrs_config::Config` whose `NAMESPACE` is
/// the argument, so the type loads itself from `NESTRS_<NAMESPACE>__*` and
/// validates on load:
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
/// Import `ConfigModule::for_feature::<DatabaseConfig>()` in a module to load it
/// once at boot and make it injectable anywhere as `Arc<DatabaseConfig>` (the
/// `ConfigType<typeof …>` + token collapse — in Rust the type *is* the token).
///
/// `#[config]` must sit **above** the derives so it sees and re-emits them intact.
/// The `namespace` must be a non-empty lowercase string (the env-domain segment).
#[proc_macro_attribute]
pub fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    config::config(args, input)
}

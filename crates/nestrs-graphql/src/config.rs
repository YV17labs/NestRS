//! [`GraphqlConfig`] — the GraphQL endpoint settings, a namespaced `#[config]`
//! loaded from `NESTRS_GRAPHQL__*` (and the `.env` cascade). Every field has a
//! **production-safe default** (playground off, SDL emit off) — the framework
//! defaults to production everywhere, as a safety. A dev run opts the tooling in
//! through `.env.development` (the development overrides), so `app.rs` carries no
//! config literal.

use std::path::PathBuf;

use nestrs_config::config;
use serde::{Deserialize, Serialize};
use validator::Validate;

pub(crate) const DEFAULT_PATH: &str = "/graphql";

#[config(namespace = "graphql")]
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(default)]
pub struct GraphqlConfig {
    /// HTTP path the schema is served at (`POST` for operations, `GET` for the
    /// playground). Default `/graphql`.
    pub path: String,
    /// Serve the GraphQL playground on `GET <path>`. Default `false`
    /// (production-safe); a dev run enables it via `NESTRS_GRAPHQL__PLAYGROUND=true`
    /// in `.env.development`.
    pub playground: bool,
    /// Where the committed SDL lives — written on boot when `emit_sdl` is `true`.
    /// Default `schema.graphql` (cwd-relative); a dev run points it at the app's
    /// committed file via `NESTRS_GRAPHQL__SCHEMA_PATH`.
    pub schema_path: PathBuf,
    /// (Re)write `schema_path` from the live schema once at boot. Drive it from
    /// the build (`cfg!(debug_assertions)`) or `NESTRS_GRAPHQL__EMIT_SDL`. A
    /// write failure is logged, never fatal. Default `false`.
    pub emit_sdl: bool,
}

impl Default for GraphqlConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_PATH.into(),
            playground: false,
            schema_path: "schema.graphql".into(),
            emit_sdl: false,
        }
    }
}

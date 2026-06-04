//! [`GraphqlConfig`] — loaded from `NESTRS_GRAPHQL__*`. Every field defaults
//! production-safe (playground off, SDL emit off); `.env.development` opts the
//! tooling in so `app.rs` carries no config literal.

use std::path::PathBuf;

use nestrs_config::{Config, ConfigService, Result, config};
use validator::Validate;

pub(crate) const DEFAULT_PATH: &str = "/graphql";

#[config(namespace = "graphql")]
#[derive(Clone, Debug, Validate)]
pub struct GraphqlConfig {
    /// Default `/graphql`.
    pub path: String,
    /// Default `false` (production-safe).
    pub playground: bool,
    /// Where the committed SDL lives. Default `schema.graphql`.
    pub schema_path: PathBuf,
    /// (Re)write `schema_path` from the live schema once at boot. Default
    /// `false`. A write failure is logged, never fatal.
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

impl Config for GraphqlConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        let d = Self::default();
        Ok(Self {
            path: env.get("PATH").unwrap_or(d.path),
            playground: env.flag("PLAYGROUND", d.playground)?,
            schema_path: env
                .get("SCHEMA_PATH")
                .map(PathBuf::from)
                .unwrap_or(d.schema_path),
            emit_sdl: env.flag("EMIT_SDL", d.emit_sdl)?,
        })
    }
}

//! [`OpenApiConfig`] — the OpenAPI document `info` block, a namespaced
//! `#[config]` loaded from `NESTRS_OPENAPI__*` (and the `.env` cascade). Every
//! field has a default, so an app imports [`OpenApiModule`](crate::OpenApiModule)
//! bare and sets its identity (`NESTRS_OPENAPI__TITLE`, `…__VERSION`,
//! `…__DESCRIPTION`) in the `.env` cascade — no config literal in `app.rs`.

use nestrs_config::config;
use serde::{Deserialize, Serialize};
use validator::Validate;

#[config(namespace = "openapi")]
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(default)]
pub struct OpenApiConfig {
    /// `info.title`.
    pub title: String,
    /// `info.version`.
    pub version: String,
    /// `info.description`, omitted when `None`.
    pub description: Option<String>,
}

impl Default for OpenApiConfig {
    fn default() -> Self {
        Self {
            title: "nestrs API".into(),
            version: "0.1.0".into(),
            description: None,
        }
    }
}

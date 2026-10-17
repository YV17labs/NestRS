//! The **hello** module — the one thing every scaffold ships.
//!
//! A freshly created project has to prove it started, so every path out of
//! `nestrs new` writes a service with a greeting and one `#[public]` `GET /`.
//! [`SERVICE`] and [`CONTROLLER`] carry no layout of their own; the `FEATURE_*`
//! wrappers below adapt them to `crates/features/src/<name>/`.

/// The greeting — a provider with one method.
pub(crate) const SERVICE: &str = r#"use nest_rs::core::injectable;

#[injectable]
#[derive(Default)]
pub struct {{service}};

impl {{service}} {
    pub fn greeting(&self) -> String {
        "Hello World".to_string()
    }
}
"#;

/// `GET /`. The service sits at `crate::<feature>::<Service>`, both halves
/// already seeded from the project name.
pub(crate) const CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::http::{controller, routes};

use crate::{{snake}}::{{service}};

#[controller(path = "/")]
pub struct {{controller}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[routes]
impl {{controller}} {
    // Every route declares a posture, and an unguarded one is flagged at boot.
    // This greeting is deliberately open, so it says so.
    #[get("/")]
    #[public]
    #[api(summary = "Greet the caller")]
    async fn hello(&self) -> String {
        self.svc.greeting()
    }
}
"#;

// The feature is named after the app it serves: an app crate keeps no
// `service.rs` / `controller.rs`.

pub(crate) const FEATURE_MOD: &str = r#"pub const TARGET: &str = "features::{{snake}}";

mod module;
mod service;

pub mod http;

pub use http::{{http_module}};
pub use module::{{module}};
pub use service::{{service}};
"#;

pub(crate) const FEATURE_MODULE: &str = r#"use nest_rs::core::module;

use super::service::{{service}};

#[module(providers = [{{service}}])]
pub struct {{module}};
"#;

pub(crate) const FEATURE_HTTP_MOD: &str = r#"mod controller;
mod module;

pub use module::{{http_module}};
"#;

pub(crate) const FEATURE_HTTP_MODULE: &str = r#"use nest_rs::core::module;

use super::controller::{{controller}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{controller}}],
)]
pub struct {{http_module}};
"#;

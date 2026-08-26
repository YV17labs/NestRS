//! The **hello** module — the one thing every scaffold ships, in every layout.
//!
//! A freshly created project has to prove it started. A `404` on `/` proves
//! nothing: the process may be healthy and the transport mounted, but the
//! developer cannot tell that from a browser. So every path out of
//! `nestrs new` — greenfield monorepo, app added to an existing workspace,
//! standalone crate — writes a service with a greeting and one `#[public]`
//! `GET /` that returns it. There is no template flag and no routeless variant:
//! this *is* the starter.
//!
//! [`SERVICE`] and [`CONTROLLER`] are layout-agnostic and shared verbatim; the
//! `FEATURE_*` wrappers below adapt them to the workspace's
//! `crates/features/src/<name>/` shape. Standalone puts the same two files
//! directly under `src/` (see [`super::standalone`]).

/// The greeting. Identical in both layouts — a provider with one method.
pub const SERVICE: &str = r#"use nest_rs::core::injectable;

#[injectable]
#[derive(Default)]
pub struct {{service}};

impl {{service}} {
    pub fn greeting(&self) -> String {
        "Hello World".to_string()
    }
}
"#;

/// `GET /`. `{{service_use}}` is the full `use` path the layout puts the
/// service at — `crate::service::HelloService` standalone,
/// `crate::hello::HelloService` in a feature — so one template serves both.
pub const CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::http::{controller, routes};

use {{service_use}};

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

// ── Workspace wrappers ───────────────────────────────────────────────────────
// The feature is named after the app it serves, because the layout keeps no
// `service.rs` / `controller.rs` in an app crate. `nestrs new hello` writes the
// `hello` feature; `nestrs new blog` inside that workspace writes the `blog`
// one — the same shape either way, so no path ends up with a mute app.

pub const FEATURE_MOD: &str = r#"mod module;
mod service;

pub mod http;

pub use http::{{http_module}};
pub use module::{{module}};
pub use service::{{service}};
"#;

pub const FEATURE_MODULE: &str = r#"use nest_rs::core::module;

use super::service::{{service}};

#[module(providers = [{{service}}])]
pub struct {{module}};
"#;

pub const FEATURE_HTTP_MOD: &str = r#"mod controller;
mod module;

pub use module::{{http_module}};
"#;

pub const FEATURE_HTTP_MODULE: &str = r#"use nest_rs::core::module;

use super::controller::{{controller}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{controller}}],
)]
pub struct {{http_module}};
"#;

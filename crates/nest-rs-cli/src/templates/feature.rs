pub const FEATURE_MOD: &str = r#"mod module;
mod service;

{{http_mod_line}}

pub use module::{{module}};
pub use service::{{service}};
{{http_pub_line}}
"#;

pub const FEATURE_MODULE: &str = r#"use nest_rs_core::module;

use super::service::{{service}};

#[module(providers = [{{service}}])]
pub struct {{module}};
"#;

pub const FEATURE_SERVICE: &str = r#"use nest_rs_core::injectable;

#[injectable]
#[derive(Default)]
pub struct {{service}};

impl {{service}} {
    pub fn count(&self) -> usize {
        0
    }
}
"#;

pub const FEATURE_HTTP_MOD: &str = r#"mod controller;
mod module;

pub use controller::{{controller}};
pub use module::{{http_module}};
"#;

pub const FEATURE_HTTP_MODULE: &str = r#"use nest_rs_core::module;

use super::controller::{{controller}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{controller}}],
)]
pub struct {{http_module}};
"#;

pub const FEATURE_HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs_http::{controller, routes};

use crate::{{snake}}::{{service}};

#[controller(path = "/{{kebab}}")]
pub struct {{controller}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[routes]
impl {{controller}} {
    #[get("/")]
    async fn list(&self) -> String {
        format!("{} items", self.svc.count())
    }
}
"#;

//! **Resource** templates — a DB-backed CRUD slice (`g resource`).
//!
//! Self-contained on purpose: an `#[expose]` entity, a `CrudService`, and an
//! HTTP adapter with explicit thin handlers that delegate to the service. No
//! authz coupling, so the slice compiles in any workspace; the next-steps
//! output points at `users/`/`orgs/` for hardening with `#[crud]` + guards.

pub const MOD: &str = r#"mod entity;
mod module;
mod service;

pub mod http;

pub use entity::*;
pub use module::{{module}};
pub use service::{{service}};

pub use http::{{{controller}}, {{http_module}}};
"#;

pub const ENTITY: &str = r#"use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "{{entity}}", service = super::service::{{service}})]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "{{table}}",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
}

impl ActiveModelBehavior for ActiveModel {}
"#;

pub const SERVICE: &str = r#"use nest_rs_core::injectable;
use nest_rs_seaorm::CrudService;

use super::entity::{{{create_input}}, Entity as {{pascal}}, {{update_input}}};

#[injectable]
#[derive(Default)]
pub struct {{service}};

impl CrudService for {{service}} {
    type Entity = {{pascal}};
    type Create = {{create_input}};
    type Update = {{update_input}};
}
"#;

pub const MODULE: &str = r#"use nest_rs_core::module;

use super::service::{{service}};

#[module(providers = [{{service}}])]
pub struct {{module}};
"#;

pub const HTTP_MOD: &str = r#"mod controller;
mod module;

pub use controller::{{controller}};
pub use module::{{http_module}};
"#;

pub const HTTP_MODULE: &str = r#"use nest_rs_core::module;

use super::controller::{{controller}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{controller}}],
)]
pub struct {{http_module}};
"#;

pub const HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs_http::{Valid, controller, routes};
use nest_rs_seaorm::{CrudService, ServiceError};
use poem::Result;
use poem::web::Json;

use crate::{{snake}}::{{{create_input}}, {{entity}}, {{service}}};

#[controller(path = "/{{kebab}}")]
pub struct {{controller}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[routes]
impl {{controller}} {
    #[get("/")]
    async fn list(&self) -> Result<Json<Vec<{{entity}}>>> {
        let rows = self.svc.list().await.map_err(ServiceError::from)?;
        Ok(Json(rows.iter().map({{entity}}::from).collect()))
    }

    #[post("/")]
    async fn create(&self, body: Valid<Json<{{create_input}}>>) -> Result<Json<{{entity}}>> {
        let model = self
            .svc
            .create(body.into_inner())
            .await
            .map_err(ServiceError::from)?;
        Ok(Json({{entity}}::from(&model)))
    }
}
"#;

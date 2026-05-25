//! The `users` table as a SeaORM entity.
//!
//! `org_id` is the multi-tenant scope the authorization rules filter on, and
//! `Serialize` lets the authz layer mask a row into a JSON response.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// `Deserialize` lets the response shaper parse the handler's JSON body back into
// a `Model` to mask it.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub email: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

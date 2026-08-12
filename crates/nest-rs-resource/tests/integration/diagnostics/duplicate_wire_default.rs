//! The masking placeholder is an audited opt-in, so two of them on one column
//! is a question about which value ships — answered by refusing, not by taking
//! the last one.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[wire_default]
    #[wire_default(String::from("redacted"))]
    pub secret: String,
}

fn main() {}

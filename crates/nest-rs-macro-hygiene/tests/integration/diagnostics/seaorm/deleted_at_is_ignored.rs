//! `soft_delete` stamps `deleted_at` and filters on it, so it has to be a
//! column: an `ignore`d field has no `Column` variant to stamp or filter.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, soft_delete)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[sea_orm(ignore)]
    pub deleted_at: Option<DateTimeWithTimeZone>,
}

fn main() {}

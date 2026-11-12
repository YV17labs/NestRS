//! `timestamps` stamps `created_at` and `updated_at` itself: a `default` there
//! would be a second declaration of one column, overwritten on `updated_at`
//! and replacing the creation time on `created_at`.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, timestamps)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub name: String,
    #[expose(default = DateTimeWithTimeZone::default())]
    pub created_at: DateTimeWithTimeZone,
}

fn main() {}

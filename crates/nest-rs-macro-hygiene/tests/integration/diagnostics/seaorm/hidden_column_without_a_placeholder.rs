//! A masked reply rebuilds each row from its exposed columns, and a hidden
//! column of a type with no placeholder — a tenant `Uuid`, a timestamp, a
//! secret of bytes — has nothing to be rebuilt from: refused at each such
//! field, naming the three ways out, where a reply carrying the row would have
//! failed with a `500`.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Post")]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    pub org_id: Uuid,
    pub published_at: DateTimeWithTimeZone,
    pub note: Option<String>,
    pub seed: nest_rs::core::Secret<Vec<u8>>,
    pub api_key_hash: nest_rs::core::Secret<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

fn main() {}

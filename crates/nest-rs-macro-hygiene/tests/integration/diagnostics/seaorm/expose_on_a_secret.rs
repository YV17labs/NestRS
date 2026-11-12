//! A `Secret` column serializes as its placeholder, so exposing one would ship
//! `"<redacted>"` where a client reads a value, and an input would write a
//! secret from the wire: the column stays off the wire, read by its service.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Account", service = AccountsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "accounts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(input(create))]
    pub api_key_hash: Option<nest_rs::core::Secret<String>>,
}

fn main() {}

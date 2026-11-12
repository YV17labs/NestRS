//! `#[expose]` reads the entity's `#[sea_orm(from = "…")]` as the field holding
//! a `HasOne` relation's foreign key, and turns it into one of the struct's
//! identifiers. A value that is not a column name is refused at SeaORM's key,
//! where it is written; it used to panic in `format_ident!`.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Post", service = PostsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[sea_orm(belongs_to, from = "org id", to = "id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}

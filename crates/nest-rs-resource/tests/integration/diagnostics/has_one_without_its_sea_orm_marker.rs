//! A `HasOne<T>` with no `belongs_to` beside it would fall through as a scalar
//! column and explode inside the `SimpleObject` derive with `HasOne does not
//! implement OutputType`, spanned on the expansion. Refuse it at the field.

use nest_rs_resource::expose;
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
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}

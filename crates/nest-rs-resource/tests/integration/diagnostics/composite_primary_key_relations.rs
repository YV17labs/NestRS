//! A composite key silently produced a single-column `by_id` loader — the wrong
//! rows on lookup, with no diagnostic at all. Until the loader takes a tuple
//! key, the refusal names the second column and the hand-rolled way out.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Membership", service = MembershipsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "memberships")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub org_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub user_id: Uuid,
    #[sea_orm(belongs_to, from = "org_id", to = "Column::Id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}

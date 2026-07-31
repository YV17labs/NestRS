//! The 1.1.x parameter order: service first, action second.

use nest_rs_authz::Read;
use nest_rs_seaorm::{Bind, CrudService};

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "swapped_bind_widgets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct WidgetsService;

impl CrudService for WidgetsService {
    type Entity = widget::Entity;
}

fn swapped(_bound: Bind<WidgetsService, Read>) {}

fn main() {}

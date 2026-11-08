//! A reply is the JSON the handler's value serializes to, whatever its return
//! type spells: a value that cannot, behind a `Result` or not, is refused at the
//! serializer's bound.

use nest_rs::authz::Read;
use nest_rs::ws::{gateway, messages};

mod note {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "unserialized_notes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct NoWireShape;

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("list")]
    #[authorize(Read, note::Entity)]
    async fn list(&self) -> NoWireShape {
        NoWireShape
    }
}

fn main() {}

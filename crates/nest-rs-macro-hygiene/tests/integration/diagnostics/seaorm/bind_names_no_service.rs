//! `bind =` names the service that loads the subject; an action marker there —
//! the action and the service swapped — is refused with the note saying so.

use nest_rs::authz::Read;
use nest_rs::http::{controller, routes};
use nest_rs::seaorm::{Authorized, CrudService};

mod widget {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Widget")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "swapped_bind_widgets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
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

#[controller(path = "/widgets")]
struct WidgetsController;

#[routes]
impl WidgetsController {
    #[get("/{id}")]
    #[authorize(Read, bind = Read)]
    async fn get(&self, widget: Authorized<Read, widget::Entity>) -> String {
        widget.id.to_string()
    }
}

fn main() {}

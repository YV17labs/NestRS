//! The wire type of `#[expose]` (`src/dto.rs` in the macro crate): every
//! exposed entity names it through `Exposed`, on every build, `graphql` or not.

use nest_rs_seaorm::{Exposed, expose};
use sea_orm::entity::prelude::*;

mod note {
    use super::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub body: String,
        pub draft: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[test]
fn an_exposed_entity_names_the_wire_type_it_generated() {
    let row = note::Model {
        id: Uuid::nil(),
        body: "hello".into(),
        draft: true,
    };
    let wire: <note::Entity as Exposed>::Wire = From::from(&row);
    let named: note::Note = wire;
    assert_eq!(named.body, "hello");
}

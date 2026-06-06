//! Defaults for `#[expose(skip)]` columns when reconciling a wire DTO back
//! into a SeaORM `Model` for response masking.

use sea_orm::EntityTrait;
use serde_json::{Map, Value};

/// Fills absent JSON keys for server-only columns before deserializing a
/// handler DTO into `Self::Model`. Emitted by `#[expose]` per skipped scalar
/// column; entities without an `#[expose]` impl get the default no-op (their
/// masking handlers must deserialize without skipped columns).
pub trait WireModelDefaults: EntityTrait {
    fn fill_wire_defaults(_map: &mut Map<String, Value>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    // The default impl is the regression sentinel: an entity that doesn't ship
    // a hand-written `WireModelDefaults` impl must leave the wire body
    // untouched — adding or renaming keys here would silently break masking.
    impl WireModelDefaults for widget::Entity {}

    #[test]
    fn default_impl_leaves_the_wire_body_untouched() {
        let mut body: Map<String, Value> = Map::new();
        body.insert("id".into(), serde_json::json!(1));
        body.insert("name".into(), serde_json::json!("ada"));

        let before = body.clone();
        widget::Entity::fill_wire_defaults(&mut body);

        assert_eq!(body, before, "default impl must not add or rename keys");
    }
}

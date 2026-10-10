//! Defaults for unexposed columns (those without `#[expose]`) when reconciling
//! a wire DTO back into a SeaORM `Model` for response masking.

use sea_orm::EntityTrait;
use serde_json::{Map, Value};

/// Fills absent JSON keys for server-only columns before deserializing a
/// handler DTO into `Self::Model`; emitted by `#[expose]`.
pub trait WireModelDefaults: EntityTrait {
    /// Insert default JSON values for the entity's unexposed columns into `map`
    /// so a wire DTO (which omits them) can deserialize into the full `Model`.
    fn fill_wire_defaults(_map: &mut Map<String, Value>) {}

    /// The exposed (`#[expose]`) column names that may cross the wire: response
    /// masking retains **only** these keys, even from a raw `Model`.
    ///
    /// `None` (the default, entities without `#[expose]`) retains the body's own
    /// keys, which is only sound when the body is already the wire shape.
    fn wire_keys() -> Option<&'static [&'static str]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

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

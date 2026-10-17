//! Defaults for unexposed columns (those without `#[expose]`) when reconciling
//! a wire DTO back into a SeaORM `Model` for response masking.

use sea_orm::{EntityTrait, IdenStatic};
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

    /// Whether [`fill_wire_defaults`](Self::fill_wire_defaults) may invent this
    /// column's value: a rule reading it cannot be decided on a rebuilt model.
    ///
    /// An impl that fills a column answers `true` for it; `#[expose]` emits the
    /// answer from the same walk that emits the fill.
    fn is_placeholdered(_column: Self::Column) -> bool {
        false
    }

    /// The key this column crosses the wire under, `None` for one the wire
    /// never carries: an object that leaves the key out did not send the
    /// column, so a rule reading it cannot be decided on the value serde gave
    /// the rebuilt model.
    ///
    /// By default, the column's own name when [`wire_keys`](Self::wire_keys)
    /// lists it, or whenever there is no such list; `#[expose]` emits the key
    /// each exposed field serializes under, which a `column_name` moves away
    /// from the column's name.
    fn wire_key(column: Self::Column) -> Option<&'static str> {
        let name = column.as_str();
        match Self::wire_keys() {
            Some(keys) => keys.iter().copied().find(|key| *key == name),
            None => Some(name),
        }
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

    mod gadget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "gadgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
            pub secret: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    impl WireModelDefaults for gadget::Entity {
        fn wire_keys() -> Option<&'static [&'static str]> {
            Some(&["id", "name"])
        }
    }

    #[test]
    fn default_impl_invents_no_column() {
        use sea_orm::Iterable;

        assert!(widget::Column::iter().all(|column| !widget::Entity::is_placeholdered(column)));
    }

    #[test]
    fn default_impl_keys_a_column_by_its_name_among_the_wire_keys() {
        assert_eq!(widget::Entity::wire_key(widget::Column::Name), Some("name"));
        assert_eq!(gadget::Entity::wire_key(gadget::Column::Name), Some("name"));
        assert_eq!(
            gadget::Entity::wire_key(gadget::Column::Secret),
            None,
            "a column outside the wire keys never crosses the wire",
        );
    }

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

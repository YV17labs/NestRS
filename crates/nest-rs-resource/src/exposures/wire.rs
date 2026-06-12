//! Defaults for unexposed columns (those without `#[expose]`) when reconciling
//! a wire DTO back into a SeaORM `Model` for response masking.

use sea_orm::EntityTrait;
use serde_json::{Map, Value};

/// Fills absent JSON keys for server-only columns before deserializing a
/// handler DTO into `Self::Model`. Emitted by `#[expose]` per unexposed scalar
/// column; entities without an `#[expose]` impl get the default no-op (their
/// masking handlers must deserialize without the hidden columns).
pub trait WireModelDefaults: EntityTrait {
    fn fill_wire_defaults(_map: &mut Map<String, Value>) {}

    /// The exposed (`#[expose]`) column names that may cross the wire. Response
    /// masking retains **only** these keys, so neither an unrestricted field
    /// grant nor a handler that returns a raw `Model` can leak an unexposed
    /// column (`password_hash`, `role`, …) — the strainer keys on the entity's
    /// statically-known exposed set rather than on whatever the response body
    /// happened to carry.
    ///
    /// `None` ⇒ the entity opted out of key-set retention (the default no-op
    /// impl, used by entities without `#[expose]`); the masker then falls back
    /// to retaining the response body's own keys, which is only sound when the
    /// body is already the wire shape.
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

//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Transport-binding tests are gated on the same feature that exposes them in
//! `src/`: run with `cargo test -p nest-rs-authz --features full` to exercise
//! every bridge in this crate.

mod ability;
mod builder;

// `src/guard.rs` is transport-agnostic, so its mirror is here and not under an
// edge; the files inside carry the per-entry feature gates.
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod guard;

#[cfg(feature = "http")]
mod http;

#[cfg(feature = "graphql")]
mod graphql;

#[cfg(feature = "mcp")]
mod mcp;

#[cfg(feature = "ws")]
mod ws;

/// A parent/child pair whose one job is to be the *wrong* relation: `child`
/// belongs_to `parent`, so a `related` call naming any other entity is the
/// mismatch that trips the fail-closed `Deny` sentinel.
///
/// At the suite root for the reason [`widget`] is: two modules assert on that
/// sentinel — the builder's own rejection and the `warn` `AbilityGuard` emits —
/// and with a copy each they could drift into testing different mismatches
/// while both reporting the rule holds.
pub(crate) mod parent {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "parents")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub org_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod child {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "children")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub parent_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(
            belongs_to = "super::parent::Entity",
            from = "Column::ParentId",
            to = "super::parent::Column::Id"
        )]
        Parent,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

/// A throwaway SeaORM entity to act as the authorization `Subject`, with a
/// server-only column (`secret`) the wire DTOs never carry —
/// [`WireModelDefaults`](nest_rs_resource::WireModelDefaults) reconstructs it so
/// policy can read it, and the exposed-key strainer drops it again.
///
/// At the suite root because the `mcp` and `ws` mirrors both mask against it: one
/// `widget` per test binary, so the two suites cannot drift into masking
/// different shapes and reporting the same conclusion.
#[cfg(any(feature = "mcp", feature = "ws"))]
pub(crate) mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "widgets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[cfg(any(feature = "mcp", feature = "ws"))]
impl nest_rs_resource::WireModelDefaults for widget::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry("secret")
            .or_insert(serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "name"])
    }
}

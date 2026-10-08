//! Integration tests mirroring `src/`; run with `--features full` to exercise
//! every transport binding.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod ability;
mod builder;

// `src/guard.rs` is transport-agnostic, so its mirror sits here, not under an edge.
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
pub(crate) mod parent {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "parents")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub org_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod child {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "children")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub parent_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {
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
#[cfg(any(feature = "mcp", feature = "ws"))]
pub(crate) mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "widgets")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

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

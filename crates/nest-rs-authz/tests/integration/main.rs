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
mod context;

// `src/guard.rs` is transport-agnostic, so its mirror sits here, not under an edge.
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod guard;

// One gate, every edge: its suite needs all four.
#[cfg(all(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod gate;

#[cfg(feature = "http")]
mod http;

// A subject no table holds, gated on every edge: its suite needs all four.
#[cfg(all(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod subject;

#[cfg(feature = "graphql")]
mod graphql;

#[cfg(feature = "mcp")]
mod mcp;

#[cfg(feature = "ws")]
mod ws;

// A wire row judged where it is built, served on every edge: its suite needs them all.
#[cfg(all(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod judged;

/// A story whose `headline` the table calls `display_title`: a field grant names the
/// column, and the wire carries it under the field's own key.
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub(crate) mod story {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Story")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "stories")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[sea_orm(column_name = "display_title")]
        #[expose]
        pub headline: String,
        #[expose(nullable)]
        pub body: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A parent/child pair whose one job is to be the *wrong* relation: `child`
/// belongs_to `parent`, so a `related` call naming any other entity is the
/// mismatch that trips the fail-closed `Deny` sentinel.
pub(crate) mod parent {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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
/// server-only column (`secret`) the wire DTOs never carry, and a `name` a grant
/// may withhold.
#[cfg(any(feature = "mcp", feature = "ws"))]
pub(crate) mod widget {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Widget", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "widgets")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose(nullable)]
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

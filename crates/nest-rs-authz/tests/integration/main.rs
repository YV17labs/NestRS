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

#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod wire_mask;

/// An org, for a post's relational rules to reach.
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub(crate) mod org {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "orgs")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A post through the real `#[expose]`: it belongs to an org, keeps a
/// server-only `deleted` flag, and exposes an `archived_at` a wire row may
/// leave out — three things a mask rebuilt from the wire cannot see.
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub(crate) mod post {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub org_id: i32,
        #[expose]
        pub title: String,
        #[expose]
        pub archived_at: Option<i64>,
        pub deleted: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {
        #[sea_orm(
            belongs_to = "super::org::Entity",
            from = "Column::OrgId",
            to = "super::org::Column::Id"
        )]
        Org,
    }

    impl ActiveModelBehavior for ActiveModel {}

    /// A post of org `id`: a rule only the relation decides.
    pub(crate) fn in_org(
        p: &nest_rs_authz::PredicateBuilder<Entity>,
        id: i32,
    ) -> nest_rs_authz::Predicate<Entity> {
        p.related::<super::org::Entity, _>(Relation::Org, |o| o.eq(super::org::Column::Id, id))
    }

    /// The wire row of post `id` in org `org_id`, as a reply or a push carries
    /// it when its type leaves `archived_at` out.
    pub(crate) fn wire(id: i32, org_id: i32) -> serde_json::Value {
        serde_json::json!({ "id": id, "org_id": org_id, "title": format!("post {id}") })
    }
}

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
        #[expose]
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
/// server-only column (`secret`) the wire DTOs never carry — `#[expose]`
/// rebuilds it from its placeholder so policy can read the row, and the
/// exposed-key strainer drops it again.
#[cfg(any(feature = "mcp", feature = "ws"))]
pub(crate) mod widget {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Widget")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "widgets")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Transport-binding tests are gated on the same feature that exposes them in
//! `src/`: run with `cargo test -p nest-rs-authz --features full` to exercise
//! every bridge in this crate.

mod ability;
mod builder;

#[cfg(feature = "http")]
mod http;

#[cfg(feature = "graphql")]
mod graphql;

#[cfg(feature = "mcp")]
mod mcp;

#[cfg(feature = "ws")]
mod ws;

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

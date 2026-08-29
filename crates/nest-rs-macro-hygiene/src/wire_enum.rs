//! `#[wire_enum]` — the enum mode of `#[expose]`.
//!
//! The sibling `#[expose]` cannot be witnessed here (it needs a real entity,
//! and `DeriveEntityModel` roots its expansion at the call site's `sea_orm`).
//! `#[wire_enum]` has no such excuse: a column's enum type is a plain Rust
//! enum, so the whole expansion — `Serialize`, `Deserialize`, `JsonSchema`,
//! `async_graphql::Enum` and their four `crate = ` overrides — has to resolve
//! against a manifest that names only the umbrella. It is precisely the derive
//! routing that was invisible to review before this file existed: written by
//! hand, an exposed enum put `schemars` **and** `async-graphql` in the entity
//! crate's manifest for code it never wrote.

use nest_rs::resource::wire_enum;

/// The wire-only form: no GraphQL surface, so no `Enum` derive and no
/// `#[graphql(crate = …)]` — a different arm of the emission from the one
/// below, and the one an HTTP-only app compiles.
#[wire_enum]
#[serde(rename_all = "snake_case")]
pub enum HygieneWireTier {
    /// A variant.
    Free,
    /// Another, so the rename actually has something to rename.
    PayAsYouGo,
}

/// The GraphQL form. `async_graphql::Enum` is the derive with no `crate = `
/// story of its own — async-graphql roots it at whatever the call site's
/// manifest declares — so this is the arm that fails here the day the override
/// is dropped.
#[wire_enum(graphql)]
pub enum HygieneStage {
    /// A variant.
    Draft,
    /// Another.
    Shipped,
}

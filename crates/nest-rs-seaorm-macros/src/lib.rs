//! `#[expose]` and `#[wire_enum]`, re-exported by `nest-rs-seaorm`.
//!
//! An *attribute* (not a derive) so it composes with `#[sea_orm::model]`, which
//! re-emits the struct and would double-expand a sibling derive.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod active;
mod attr;
mod dto;
mod expose;
mod input;
mod lifecycle;
mod relations;
mod wire;
mod wire_enum;

/// Emits a wire DTO (`Serialize` + `JsonSchema`) and `Create/Update` input
/// types; add the `graphql` flag (and enable the `graphql` feature on
/// `nest-rs-seaorm`) for GraphQL types and auto-resolved relations. Add
/// `soft_delete` and/or `timestamps` for lifecycle columns (see
/// `nest-rs-seaorm` `SoftDeletable` + `CrudService::soft_delete_column`).
///
/// **Exposure is opt-in.** A column crosses the wire only when it carries
/// `#[expose]`; a field with no `#[expose]` is hidden from every transport
/// (HTTP, GraphQL, WS). `#[expose(input(...))]` opts the field into the write
/// DTOs *and* implies read.
///
/// Generates `User`, `CreateUser`, `UpdateUser`, `From<&Model> for User`.
///
/// # Expands to
///
/// The original entity unchanged, plus: the wire DTO (`Serialize` +
/// `JsonSchema`, GraphQL `SimpleObject` under `graphql`), the `Create`/`Update`
/// input types, active-model write glue, `impl WireModelDefaults` (for response
/// masking to rebuild unexposed columns), lifecycle column glue
/// (`soft_delete`/`timestamps`), and — under `graphql` — the relation loaders +
/// `#[ComplexObject]` field resolvers for `#[expose]`d relations.
#[proc_macro_attribute]
pub fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(expose::expose(args, item).into()).into()
}

/// Makes a column's custom enum a wire type: `Serialize`, `Deserialize`,
/// `JsonSchema` and — under `graphql` — `async_graphql::Enum`, routed through
/// `nest-rs-seaorm` so the entity crate's manifest names none of them.
///
/// It emits the value shape those derives require — `Clone`, `Copy`, `Debug`,
/// `PartialEq`, `Eq` — and **nothing from SeaORM**: `EnumIter`,
/// `DeriveActiveEnum`, `#[sea_orm(rs_type = …, db_type = …)]` and the
/// per-variant `string_value` stay the developer's.
///
/// # Expands to
///
/// The enum unchanged, under the derives above, each with its `crate = `
/// override (`serde`, `schemars`, and `graphql` for `Enum`).
///
/// Drop `graphql` for an enum that only ever crosses HTTP; like
/// `#[expose(…, graphql)]`, it needs the `graphql` feature on `nest-rs-seaorm`.
#[proc_macro_attribute]
pub fn wire_enum(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(wire_enum::wire_enum(args.into(), item.into())).into()
}

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
/// `soft_delete` and/or `timestamps` for lifecycle columns: `soft_delete`
/// registers the entity, so a delete stamps `deleted_at` and every `Repo` read
/// hides a stamped row (see `nest-rs-seaorm`'s `SoftDeletable`); `timestamps`
/// emits the entity's `ActiveModelBehavior`, which stamps `created_at` on
/// insert and, when the entity has one, `updated_at` on every save. Without
/// `timestamps` the entity writes its own, empty when it needs no hook.
///
/// **An insert through `Repo` fills the entity's column defaults** the row
/// left unset: a `Uuid` primary key with a v7 UUID, and a column's
/// `#[expose(default = expr)]` — a server-side constant such as a draft
/// status, on a column the create input does not carry. The decorator
/// registers them, as `soft_delete` registers its tombstone, so the entity's
/// own hooks stay its own.
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
/// input types, active-model write glue, the round trip a masked reply runs on
/// a row — each column through its own serde impls, never a `Serialize` on the
/// `Model` — lifecycle glue (the column defaults' registration, `soft_delete`'s
/// `SoftDeletable` and registration, `timestamps`' `ActiveModelBehavior`), and
/// — under `graphql` — the relation loaders + `#[ComplexObject]` field
/// resolvers for `#[expose]`d relations.
///
/// A `Serialize` or `Deserialize` derived on the row — in `#[derive]` or
/// sea-orm's `model_attrs` and `model_ex_attrs` — is refused at the derive: the
/// row crosses a wire only as its exposed type, and a write only as its inputs.
///
/// A masked reply rebuilds every hidden column, so each one needs a
/// placeholder: a `String`, `Option`, `bool` or number takes its `Default`, a
/// `Secret` its value's, and any other takes `#[wire_default]` (its type's
/// `Default`) or `#[wire_default(expr)]`. A hidden column with none, and a
/// hidden primary key, are refused at the field.
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

//! `#[expose]`, re-exported by `nest-rs-seaorm`.
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
mod refused_wires;
mod relations;

/// Emits a wire DTO (`Serialize` + `JsonSchema`) and `Create/Update` input
/// types; add the `graphql` flag (and enable the `graphql` feature on the
/// umbrella, `nest-rs`) for GraphQL types and auto-resolved relations. Add
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
/// **The wire row is judged where it is built.** `From<&Model> for User` reads
/// the row under the ability installed there, at `Read`: a column the caller's
/// grant withholds is not in the wire row at all, a row its grants refuse
/// answers no one, and the row carries who judged it, so it serializes, and
/// resolves on GraphQL, for that caller alone. A grant may withhold only a
/// column its wire type answers `null`: an `Option` column, or one
/// `#[expose(nullable)]` declares, whose wire field is an `Option` and whose
/// schema is nullable on every edge, its key always written. A grant on `Read`
/// withholding any other is refused when the ability builds.
///
/// Generates `User`, `CreateUser`, `UpdateUser`, `From<&Model> for User`.
///
/// # Expands to
///
/// The original entity unchanged, plus: the wire DTO (`Serialize`, written by
/// hand around the judgement's proof, and `JsonSchema`, GraphQL `SimpleObject`
/// under `graphql`, each column guarded by that proof), the judged conversion,
/// the columns no grant may withhold, filed for the ability builder, the
/// `Create`/`Update` input types, active-model write glue, lifecycle glue (the
/// column defaults' registration, `soft_delete`'s `SoftDeletable` and
/// registration, `timestamps`' `ActiveModelBehavior`), and — under `graphql` —
/// the relation loaders + `#[ComplexObject]` field resolvers for `#[expose]`d
/// relations, a `belongs_to` reading its link off the judged row.
///
/// The row crosses a wire only as its exposed type, and a write only as its
/// inputs. A `Serialize`, `Deserialize`, `JsonSchema`, `SimpleObject` or
/// `InputObject` derived on it — in `#[derive]` or sea-orm's `model_attrs` and
/// `model_ex_attrs` — is refused at the derive, and the expansion's own impls of
/// those traits, on the `Model` and on the `ModelEx` beside it (known by the name
/// of `#[sea_orm::model]`, written under the decorator), overlap one written by
/// hand or derived above the decorator and report what to answer where one is used.
#[proc_macro_attribute]
pub fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(expose::expose(args, item).into()).into()
}

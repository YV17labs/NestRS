//! `#[expose]`, re-exported by `nestrs-resource`.
//!
//! An *attribute* (not a derive) so it composes with `#[sea_orm::model]`, which
//! re-emits the struct and would double-expand a sibling derive.

use proc_macro::TokenStream;

mod active;
mod attr;
mod dto;
mod expose;
mod input;
mod lifecycle;
mod relations;
mod wire;

/// Expose a SeaORM entity to REST/OpenAPI (and optionally GraphQL) from one
/// declaration. Emits a wire DTO (`Serialize` + `JsonSchema`) and
/// `Create/Update` input types; add the `graphql` flag (and enable the
/// `graphql` feature on `nest-rs-resource`) for GraphQL types and
/// auto-resolved relations. Add `soft_delete` and/or `timestamps` for lifecycle
/// columns (see `nest-rs-seaorm` `SoftDeletable` + `CrudService::soft_delete_column`).
///
/// **Exposure is opt-in.** A column crosses the wire only when it carries
/// `#[expose]`; a field with no `#[expose]` is hidden from every transport
/// (HTTP, GraphQL, WS). `#[expose(input(...))]` opts the field into the write
/// DTOs *and* implies read. The payoff is fail-secure evolution: a column added
/// by a later migration stays invisible until someone deliberately exposes it —
/// no `mfa_secret` ever leaks by omission.
///
/// ```ignore
/// #[expose(name = "User", service = super::service::UsersService)]
/// #[expose(name = "User", service = super::service::UsersService, graphql)]
/// #[expose(name = "User", service = super::service::UsersService, soft_delete, timestamps)]
/// #[sea_orm::model]
/// pub struct Model {
///     #[sea_orm(primary_key, auto_increment = false)]
///     #[expose]                                                  // read-only
///     pub id: Uuid,
///     #[expose]                                                  // read-only
///     pub org_id: Uuid,
///     #[expose(input(create, update), validate(length(min = 1)))] // read + write
///     pub name: String,
///     #[expose(input(create), validate(email))]                  // read + create-only
///     pub email: String,
///     pub password_hash: Option<String>,                         // no #[expose] ⇒ hidden
/// }
/// ```
///
/// Generates `User`, `CreateUser`, `UpdateUser`, `From<&Model> for
/// User`. Adding `paginate` also emits `UserPage`.
///
/// # Expands to
///
/// The original entity unchanged, plus: the wire DTO (`Serialize` +
/// `JsonSchema`, GraphQL `SimpleObject` under `graphql`), the `Create`/`Update`
/// input types, active-model write glue, `impl WireModelDefaults` (for response
/// masking to rebuild unexposed columns), lifecycle column glue
/// (`soft_delete`/`timestamps`), and — under `graphql` — the relation loaders +
/// `#[ComplexObject]` field resolvers for `#[expose]`d relations.
///
/// ```ignore
/// pub struct Model { /* the entity, unchanged */ }
///
/// pub struct User { pub id: Uuid, pub name: String, /* #[expose]d columns only */ }
/// impl From<&Model> for User { /* … */ }
/// pub struct CreateUser { /* #[expose(input(create))] columns */ }
/// pub struct UpdateUser { /* #[expose(input(update))] columns */ }
/// impl ::nest_rs_seaorm::WireModelDefaults for Entity { /* defaults for unexposed columns */ }
/// // graphql: relation PK/FK loaders + `#[ComplexObject] impl User { … }`
/// // paginate: `pub struct UserPage { … }`
/// ```
#[proc_macro_attribute]
pub fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    expose::expose(args, item)
}

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
mod relations;
mod wire;

/// Expose a SeaORM entity to REST/OpenAPI (and optionally GraphQL) from one
/// declaration. Emits a wire DTO (`Serialize` + `JsonSchema`) and
/// `Create/Update` input types; add the `graphql` flag (and enable the
/// `graphql` feature on `nest-rs-resource`) for GraphQL types and
/// auto-resolved relations.
///
/// ```ignore
/// #[expose(name = "User", service = super::service::UsersService)]
/// #[expose(name = "User", service = super::service::UsersService, graphql)]
/// #[sea_orm::model]
/// pub struct Model {
///     #[sea_orm(primary_key, auto_increment = false)]
///     pub id: Uuid,
///     #[expose(skip)]
///     pub org_id: Uuid,
///     #[expose(input(create, update), validate(length(min = 1)))]
///     pub name: String,
///     #[expose(input(create), validate(email))]
///     pub email: String,
/// }
/// ```
///
/// Generates `User`, `CreateUserInput`, `UpdateUserInput`, `From<&Model> for
/// User`. Adding `paginate` also emits `UserPage`.
#[proc_macro_attribute]
pub fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    expose::expose(args, item)
}

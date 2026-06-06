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
mod wire;

/// Expose a SeaORM entity to GraphQL + OpenAPI from one declaration. Emits a
/// GraphQL output object (`SimpleObject` + `JsonSchema`) and `Create/Update`
/// input types; re-emits the entity untouched so the ORM macros keep full
/// power.
///
/// ```ignore
/// #[expose(name = "User")]
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

//! `#[expose]`, re-exported by `nestrs-resource`. Placed on a SeaORM entity, it
//! exposes the entity to GraphQL **and** OpenAPI from one declaration: it emits
//! a GraphQL output object (`SimpleObject` + `JsonSchema`) and the
//! `Create/Update` input types, then re-emits the entity untouched so the ORM
//! macros (`#[sea_orm::model]` or `DeriveEntityModel`) keep their full power.
//!
//! It is an *attribute* (not a derive) precisely so it composes with
//! `#[sea_orm::model]`, which re-emits the struct and would double-expand a
//! sibling derive. It declares only how the *type* is exposed — never routes or
//! guards, which belong on controllers/resolvers.
//!
//! ```ignore
//! #[expose(name = "User")]
//! #[sea_orm::model]
//! #[sea_orm(table_name = "user")]
//! pub struct Model {
//!     #[sea_orm(primary_key, auto_increment = false)]
//!     pub id: Uuid,
//!     #[expose(skip)]                                  // server-only column
//!     pub org_id: Uuid,
//!     #[expose(input(create, update), validate(length(min = 1)))]
//!     pub name: String,
//!     #[expose(input(create), validate(email))]
//!     pub email: String,
//! }
//! ```
//!
//! generates `User` (GraphQL object + JSON schema), `CreateUserInput`,
//! `UpdateUserInput`, and `From<&Model> for User`. The resolver and controller
//! (hand-written, with their own `#[query]`/`#[get]`/`#[use_guards]`) consume
//! these types.

use proc_macro::TokenStream;

mod active;
mod attr;
mod dto;
mod expose;
mod input;

/// Expose a SeaORM entity to GraphQL + OpenAPI. See the crate docs for grammar.
#[proc_macro_attribute]
pub fn expose(args: TokenStream, item: TokenStream) -> TokenStream {
    expose::expose(args, item)
}

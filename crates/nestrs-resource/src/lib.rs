//! Expose a SeaORM entity to GraphQL **and** OpenAPI from one declaration.
//!
//! Put [`macro@expose`] on a SeaORM entity to generate its GraphQL output
//! object (`SimpleObject` + `JsonSchema`) and `Create/Update` input types, while
//! leaving the entity itself untouched — so it keeps the ORM's full power
//! (`#[sea_orm::model]` or `DeriveEntityModel`). Routes and guards stay where
//! they belong: on hand-written controllers (`#[controller]`/`#[routes]`) and
//! resolvers (`#[resolver]`), which consume the generated types.
//!
//! Adding `paginate` to `#[expose(name = "User", paginate)]` additionally emits
//! a `UserPage` envelope (`{ items, total, page, per_page, total_pages,
//! has_next_page, has_previous_page }`) for both surfaces; pair it with the
//! shared [`PageArgs`] request type. Relations are *not* auto-generated: a
//! related field is a hand-written `#[field]` resolver backed by a
//! `#[dataloader]` on the data layer (the framework's batched, N+1-free pattern),
//! which the macro deliberately leaves to the resolver — see the crate `ROADMAP`.

mod pagination;
mod wire;

pub use nestrs_resource_macros::expose;
pub use pagination::PageArgs;
pub use wire::WireModelDefaults;

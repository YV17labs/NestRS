//! Expose a SeaORM entity to GraphQL **and** OpenAPI from one declaration.
//!
//! [`macro@expose`] generates the GraphQL output object (`SimpleObject` +
//! `JsonSchema`) and `Create/Update` input types from a SeaORM entity. Adding
//! `paginate` emits a `<Name>Page` envelope on both surfaces, paired with the
//! shared [`PageArgs`] request type.
//!
//! **Relations** declared with `#[sea_orm(belongs_to, …)]` / `#[sea_orm(has_many)]`
//! and **not** marked `#[expose(skip)]` are auto-exposed as GraphQL fields,
//! backed by per-request dataloaders that respect the ambient `Ability` —
//! `belongs_to` resolves via [`PkLoadable`]; `has_many` via [`RelatedTo`]
//! (Phase 2). Opt out per relation with `#[expose(skip)]` and write a manual
//! `#[field_resolver]` if a custom resolver is needed.

mod pagination;
mod relations;
mod wire;

pub use nest_rs_resource_macros::expose;
pub use pagination::PageArgs;
pub use relations::{PkLoadable, RelatedTo};
pub use wire::WireModelDefaults;

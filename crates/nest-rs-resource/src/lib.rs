//! Expose a SeaORM entity to GraphQL **and** OpenAPI from one declaration.
//!
//! [`macro@expose`] generates the GraphQL output object (`SimpleObject` +
//! `JsonSchema`) and `Create/Update` input types from a SeaORM entity. Adding
//! `paginate` emits a `<Name>Page` envelope on both surfaces, paired with the
//! shared [`PageArgs`] request type. Relations are not auto-generated —
//! hand-write `#[field]` resolvers backed by `#[dataloader]`s.

mod pagination;
mod wire;

pub use nest_rs_resource_macros::expose;
pub use pagination::PageArgs;
pub use wire::WireModelDefaults;

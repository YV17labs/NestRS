//! Expose a SeaORM entity to REST/OpenAPI from one declaration via [`macro@expose`].
//!
//! The wire DTO (`Serialize` + `JsonSchema`), CRUD input types, and
//! [`WireModelDefaults`] for response masking are always emitted.
//! Add the `graphql` flag on `#[expose(...)]` **and** enable the `graphql`
//! feature on this crate to also emit GraphQL types, auto-resolved relations,
//! and dataloaders.
//!
//! **Exposure is opt-in:** a column reaches the wire only when its field
//! carries `#[expose]` (or `#[expose(input(...))]`, which implies read). A field
//! with no `#[expose]` stays hidden on every transport, so a column added by a
//! later migration never leaks by omission.
//!
//! ```ignore
//! // HTTP / OpenAPI / masking — no GraphQL deps in the entity crate.
//! #[expose(name = "Item", service = super::service::ItemsService)]
//!
//! // + GraphQL surface (requires `features = ["graphql"]` on `nest-rs-resource`).
//! #[expose(name = "Item", service = super::service::ItemsService, graphql)]
//! ```
#![warn(missing_docs)]

mod exposures;

/// Re-exports of the `async-graphql` primitives `#[expose(..., graphql)]`
/// emits, so generated code names them through this crate.
#[cfg(feature = "graphql")]
pub mod graphql {
    pub use nest_rs_graphql::async_graphql;
    pub use nest_rs_graphql::dataloader;
}

#[cfg(feature = "graphql")]
pub use exposures::relations::{PkLoadable, RelatedTo};
pub use exposures::wire::WireModelDefaults;
pub use nest_rs_resource_macros::expose;

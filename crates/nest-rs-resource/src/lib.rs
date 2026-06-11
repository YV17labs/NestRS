//! Expose a SeaORM entity to REST/OpenAPI from one declaration via [`macro@expose`].
//!
//! The wire DTO (`Serialize` + `JsonSchema`), CRUD input types, pagination
//! envelope, and [`WireModelDefaults`] for response masking are always emitted.
//! Add the `graphql` flag on `#[expose(...)]` **and** enable the `graphql`
//! feature on this crate to also emit GraphQL types, auto-resolved relations,
//! and dataloaders.
//!
//! ```ignore
//! // HTTP / OpenAPI / masking — no GraphQL deps in the entity crate.
//! #[expose(name = "Item", service = super::service::ItemsService)]
//!
//! // + GraphQL surface (requires `features = ["graphql"]` on `nest-rs-resource`).
//! #[expose(name = "Item", service = super::service::ItemsService, graphql)]
//! ```

mod exposures;

#[cfg(feature = "graphql")]
pub mod graphql {
    pub use nest_rs_graphql::async_graphql;
    pub use nest_rs_graphql::dataloader;
}

pub use exposures::pagination::PageArgs;
#[cfg(feature = "graphql")]
pub use exposures::relations::{PkLoadable, RelatedTo};
pub use exposures::wire::WireModelDefaults;
pub use nest_rs_resource_macros::expose;

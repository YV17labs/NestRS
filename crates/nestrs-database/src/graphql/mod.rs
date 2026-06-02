//! GraphQL transport bindings for the data layer. Enabled by the `graphql`
//! Cargo feature.
//!
//! - [`bind`] — by-id route-model binding for resolvers (the analog of
//!   `nestrs_authz::http::Bind`).
//! - [`LoaderScope`] — re-installs the request's ambient executor and ability
//!   inside each `#[dataloader]` batch (implements `nestrs-graphql`'s
//!   `BatchContext` seam).
//!
//! These live in `nestrs-database` rather than `nestrs-authz` because the
//! engine cannot depend on the data layer (`nestrs-database` already depends
//! on `nestrs-authz`). They are nonetheless the data-side counterparts to the
//! authz bindings in `nestrs_authz::graphql`.

mod bind;
mod loader;

pub use bind::bind;
pub use loader::LoaderScope;

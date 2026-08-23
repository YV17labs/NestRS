//! GraphQL data-layer bindings (feature `graphql`). [`bind`] / [`bind_required`]
//! are the resolver analog of [`crate::Bind`]; [`LoaderScope`]
//! re-installs the ambient executor and ability inside each `#[dataloader]`
//! batch. They live here rather than `nest-rs-authz` because the engine cannot
//! depend on the data layer.

mod bind;
mod loader;

pub use bind::{bind, bind_required, parse_v7};
pub use loader::LoaderScope;

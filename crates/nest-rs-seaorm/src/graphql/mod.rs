//! GraphQL data-layer bindings (feature `graphql`). [`bind`] / [`bind_required`]
//! are the resolver analog of `nest_rs_authz::http::Bind`; [`LoaderScope`]
//! re-installs the ambient executor and ability inside each `#[dataloader]`
//! batch. They live here rather than `nestrs-authz` because the engine cannot
//! depend on the data layer.

mod bind;
mod loader;

pub use bind::{bind, bind_required, bind_required_with};
pub use loader::LoaderScope;

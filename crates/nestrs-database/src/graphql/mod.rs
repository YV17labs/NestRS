//! GraphQL data-layer bindings (feature `graphql`). [`bind`] is the resolver
//! analog of `nestrs_authz::http::Bind`; [`LoaderScope`] re-installs the
//! ambient executor and ability inside each `#[dataloader]` batch. They live
//! here rather than `nestrs-authz` because the engine cannot depend on the
//! data layer.

mod bind;
mod loader;

pub use bind::bind;
pub use loader::LoaderScope;

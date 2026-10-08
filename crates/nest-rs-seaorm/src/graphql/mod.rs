//! GraphQL data-layer bindings (feature `graphql`): [`bind`](fn@bind) /
//! [`bind_required`], the resolver analog of [`crate::Bind`], and [`LoaderScope`].

mod bind;
mod loader;

pub use bind::{bind, bind_required, parse_v7};
pub use loader::LoaderScope;

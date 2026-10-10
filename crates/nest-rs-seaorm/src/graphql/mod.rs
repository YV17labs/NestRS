//! GraphQL data-layer bindings (feature `graphql`): [`bind`](fn@bind) /
//! [`bind_required`], the resolver analog of [`crate::Bind`], and [`LoaderScope`];
//! and the relation bridges `#[expose(…, graphql)]` implements, so one
//! entity's field resolver reaches another entity's loader.

mod bind;
mod loader;
mod relations;

pub use bind::{bind, bind_required, parse_v7};
pub use loader::LoaderScope;
pub use relations::{PkLoadable, RelatedTo, RelationKey, RelationPage, SoleForeignKey};

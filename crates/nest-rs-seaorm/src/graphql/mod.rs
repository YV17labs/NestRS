//! GraphQL data-layer bindings (feature `graphql`): [`bind`](fn@bind) /
//! [`bind_required`], the resolver analog of [`crate::Bind`]; and the relation
//! bridges `#[expose(…, graphql)]` implements, so one entity's field resolver
//! reaches another entity's loader.
//!
//! A DataLoader batch needs no binding: it runs on the pool, under the caller's
//! ability, through what its request carries into a `Batch` — a relation
//! resolved in a mutation's own response reads the state before the mutation.

mod bind;
mod relations;

pub use bind::{bind, bind_required, parse_v7};
pub use relations::{PkLoadable, RelatedTo, RelationKey, RelationPage, SoleForeignKey};

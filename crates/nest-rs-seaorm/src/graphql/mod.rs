//! GraphQL data-layer bindings (feature `graphql`): [`parse_v7`], the by-id
//! argument's parser; and the relation bridges `#[expose(…, graphql)]`
//! implements, so one entity's field resolver reaches another entity's loader.
//! An operation loads its subject by declaring `#[authorize(A, bind = S)]`.
//!
//! A DataLoader batch needs no binding: it runs on the pool, under the caller's
//! ability, through what its request carries into a `Batch` — a relation
//! resolved in a mutation's own response reads the state before the mutation. It
//! filters rows, and each wire row it builds is judged for the caller there, so
//! every alias one batch serves reads what that caller may.

pub(crate) mod bind;
pub(crate) mod relations;

pub use bind::parse_v7;
pub use relations::{PkLoadable, RelatedTo, RelationKey, RelationPage, SoleForeignKey};

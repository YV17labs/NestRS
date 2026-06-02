//! The product's shared functional core — entities, services, auth policy, and GraphQL field resolvers.
//!
//! Business logic lives here once; apps under `apps/` attach transports and declare endpoints.
//! See `CLAUDE.md` for the domain vs app split.

pub mod authn;
pub mod authz;
pub mod identity;
pub mod oauth;
pub mod orgs;
pub mod users;

pub use identity::{Claims, Role};

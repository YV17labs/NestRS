//! The product's feature library — vertical slices as port + adapter modules.
//!
//! Every feature lives under `<feature>/` with a `core/` (the service-port:
//! entity, service, DTOs) and one sub-folder per adapter (`http/`,
//! `graphql/`, `ws/`, `queue/`, `mcp/`), each declaring its own `#[module]`.
//! An app under `apps/` composes by listing exactly the edge modules it serves.
//! See `CLAUDE.md` for the layout rules.

pub mod authn;
pub mod authz;
pub mod identity;
pub mod oauth;
pub mod orgs;
pub mod users;

pub use identity::{Claims, Role};

//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps: `authn/`, `authz/` DI modules; `oauth/strategy.rs` (Poem HTTP).

mod authz;
mod oauth;
mod orgs;
mod users;

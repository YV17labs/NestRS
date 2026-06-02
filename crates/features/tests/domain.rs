//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps: `authn/` and `authz/` DI modules; `oauth/strategy.rs`
//! (Poem HTTP). See each submodule's `mod.rs` for the pointer to where the
//! behaviour is actually exercised.

mod authn;
mod authz;
mod oauth;
mod orgs;
mod users;

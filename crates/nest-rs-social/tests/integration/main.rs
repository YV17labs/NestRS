//! Integration coverage for the social provider seam: link-time entry
//! submission and the flow-owning trait's default delegation.
//!
//! Reachability filtering and the fail-boot validation rules are unit-tested
//! inside `src/registry.rs` (they need the crate-private `install`).

mod provider;
mod providers;

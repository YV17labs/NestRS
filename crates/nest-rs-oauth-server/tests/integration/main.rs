//! Integration tests for `nest-rs-oauth-server`. Layout mirrors `src/`.
//!
//! `registry.rs` is covered by the unit tests beside it: constant-time
//! comparison and the miss paths are properties of the function, not of a
//! mounted endpoint, so a boot would add nothing an in-file test cannot see.

mod error;
mod token;

//! Integration tests for `nestrs-authn`. Layout strictly mirrors `src/` (see CLAUDE.md).
//!
//! - This file is the only `tests/*.rs` binary; paths under `tests/` are modules.
//! - Shared helpers: `tests/common/` (the only path without a `src/` counterpart).
//! - Documented gaps: `jwt/module.rs`, `oauth/module.rs`; app e2e for live HTTP.

mod common;
mod error;
mod jwt;
mod oauth;
mod passport;
mod password;

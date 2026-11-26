//! `nest-rs-reqwest` in process: real requests from a reqwest client — through
//! the middleware, under its feature — to a TLS peer double whose certificate a
//! `TestAuthority` issued, which keeps the head of every request it reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

#[cfg(feature = "middleware")]
mod middleware;
mod safe_link;

use std::sync::LazyLock;

use nest_rs_testing::{TestAuthority, TestCertificate};
// The crate's `SafeLink`s are linked only where a path names it, which nothing
// else here does without `middleware`.
use nest_rs_reqwest as _;

/// The authority the peer's certificate chains to, and the client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

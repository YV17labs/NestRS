//! Live presign round-trip against an S3-compatible server (RustFS in the dev
//! container). Proves that `object_store`'s `Signer` produces URLs a plain HTTP
//! client can PUT to and GET from, in path-style over plain HTTP.
//!
//! Config starts from `StorageConfig::default()`, which targets the dev
//! container's RustFS (`http://rustfs:9000`, `nestrs`/`nestrs`, bucket
//! `nestrs`, path-style). The endpoint honors the documented
//! `NESTRS_STORAGE__ENDPOINT` override so the round-trip can point at a server
//! outside the dev container; unset, it falls back to the default.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod client;

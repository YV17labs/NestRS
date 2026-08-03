//! Live presign round-trip against an S3-compatible server (RustFS in the dev
//! container). Proves that `object_store`'s `Signer` produces URLs a plain HTTP
//! client can PUT to and GET from, in path-style over plain HTTP.
//!
//! Needs a reachable server — gated out of `unit` by the nextest `binary(e2e)`
//! filter. Run it explicitly:
//!
//! ```bash
//! cargo nextest run -p nest-rs-storage -E 'binary(e2e)'
//! ```
//!
//! Config starts from `StorageConfig::default()`, which targets the dev
//! container's RustFS (`http://rustfs:9000`, `nestrs`/`nestrs`, bucket
//! `nestrs`, path-style). The endpoint honors the documented
//! `NESTRS_STORAGE__ENDPOINT` override so the round-trip can point at a server
//! outside the dev container; unset, it falls back to the default.

mod client;

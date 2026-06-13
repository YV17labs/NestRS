//! S3-compatible object storage for nestrs.
//!
//! A thin, injectable [`Storage`] client over the
//! [`object_store`](https://docs.rs/object_store) crate — the generic
//! object-store abstraction maintained under Apache Arrow. Infra only: this
//! crate holds no domain entity, just the bytes-and-URLs seam that feature
//! modules (presigned uploads, media variants) build on.
//!
//! ## Why `object_store`
//!
//! `object_store` is **multi-driver** (S3, GCS, Azure, local filesystem,
//! in-memory) behind one [`ObjectStore`](object_store::ObjectStore) trait, and
//! its presigning lives in a separate [`Signer`](object_store::signer::Signer)
//! trait implemented by the S3 driver. It is **reqwest/rustls-based** with no
//! `aws-runtime`/`aws-sdk-*` in its tree, so it builds cleanly on the
//! workspace's pinned Rust toolchain (where the official AWS SDK currently does
//! not). This crate wires the [`AmazonS3`](object_store::aws::AmazonS3) driver
//! by default; pointing at GCS/Azure/fs later is a builder change in
//! [`Storage`], not an API change for consumers.
//!
//! ## Usage
//!
//! Import [`StorageModule`] at the composition root. It owns its
//! [`StorageConfig`] (namespace `storage`, loaded from `NESTRS_STORAGE__*`) and
//! registers [`Storage`] as an injectable provider:
//!
//! ```ignore
//! use nest_rs_storage::{Storage, StorageModule};
//!
//! // in a feature service:
//! #[inject] storage: Arc<Storage>,
//!
//! let url = storage.presign_put("uploads/abc", Duration::from_secs(900)).await?;
//! ```
//!
//! ## API surface
//!
//! - [`Storage::presign_put`] / [`Storage::presign_get`] — short-lived signed
//!   URLs handed to a client to upload/download directly.
//! - [`Storage::head`] — size of an uploaded object (`None` if absent).
//!   `object_store` does not expose the stored `Content-Type`, so [`HeadMetadata`]
//!   carries only the byte size.
//! - [`Storage::get_bytes`] / [`Storage::put_bytes`] — server-side byte
//!   read/write (e.g. a worker transforming an original).

mod client;
mod config;
mod error;
mod module;

pub use client::{HeadMetadata, Storage};
pub use config::StorageConfig;
pub use error::{Result, StorageError};
pub use module::StorageModule;

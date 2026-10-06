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
//! [`StorageConfig`] (namespace `storage`, loaded from `<PREFIX>_STORAGE__*`) and
//! registers [`Storage`] as an injectable provider:
//!
//! ```
//! # use std::sync::Arc;
//! # use std::time::Duration;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_testing::TestApp;
//! use nest_rs_storage::{Storage, StorageModule};
//!
//! // in a feature service:
//! #[injectable]
//! struct UploadsService {
//!     #[inject]
//!     storage: Arc<Storage>,
//! }
//!
//! #[module(imports = [StorageModule], providers = [UploadsService])]
//! struct UploadsModule;
//! # #[nest_rs_core::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let app = TestApp::builder().module::<UploadsModule>().build_headless().await?;
//! # let UploadsService { storage } = &*app.container().get::<UploadsService>().expect("provided");
//!
//! let url = storage.presign_put("uploads/abc", Duration::from_secs(900)).await?;
//! assert!(url.contains("/uploads/abc?") && url.contains("X-Amz-Expires=900"));
//! # Ok(())
//! # }
//! ```
//!
//! ## API surface
//!
//! - [`Storage::presign_put`] / [`Storage::presign_get`] — short-lived signed
//!   URLs handed to a client to upload/download directly.
//! - [`Storage::head`] — size of an uploaded object (`None` if absent).
//!   `object_store` does not expose the stored `Content-Type`, so [`ObjectMetadata`]
//!   carries only the byte size.
//! - [`Storage::get_bytes`] / [`Storage::put_bytes`] — server-side byte
//!   read/write (e.g. a worker transforming an original).
//! - [`Storage::get_stream`] / [`Storage::put_stream`] — the same read/write
//!   without holding the object whole, for bodies larger than memory.
//! - [`Storage::list`] — the objects under a prefix, streamed as
//!   [`ObjectEntry`] values.
//!
//! ## How long a call waits
//!
//! Two bounds, named for the AWS SDK's: a call waits for S3's answer within
//! [`StorageConfig::operation_timeout`], every retry included — an upload's body
//! is part of its request, so [`put_stream`](Storage::put_stream) ships parts
//! that each fit it — and past it fails as its own error, naming the budget. A
//! download's body is a transfer, never cut for its size: one that stalls past
//! [`StorageConfig::read_timeout`] is cut and resumed from where it stopped,
//! within `object_store`'s retries (3 minutes from the call). The operation
//! timeout is the client's `nest_rs_core::Budget`, so the boot refuses it at or
//! past a net reaching the client.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — Object storage operations.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::storage";

mod client;
mod config;
mod error;
mod module;

#[doc(hidden)]
pub use client::MULTIPART_PART_SIZE;
pub use client::{ObjectEntry, ObjectMetadata, Storage};
pub use config::StorageConfig;
pub use error::{Result, StorageError};
pub use module::{StorageModule, StorageSetup};

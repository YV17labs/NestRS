//! S3-compatible object storage for nestrs.
//!
//! A thin, injectable [`Storage`] client over the
//! [`object_store`](https://docs.rs/object_store) crate's
//! [`AmazonS3`](object_store::aws::AmazonS3) driver.
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
//! A call waits for S3's answer within [`StorageConfig::operation_timeout`],
//! every retry included, and past it fails naming the budget;
//! [`put_stream`](Storage::put_stream) ships parts that each fit it. A
//! download's body is never cut for its size: one that sends nothing for
//! [`StorageConfig::read_timeout`] is resumed from where it stopped, and fails
//! naming the bound when the resumed body sends nothing within it. The boot
//! refuses an operation timeout at or past a net reaching the client.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — Object storage operations.
pub const TARGET: &str = "nest_rs::storage";

mod client;
mod config;
mod error;
mod module;
mod tls;
mod transfer;

#[doc(hidden)]
pub use client::MULTIPART_PART_SIZE;
pub use client::{ObjectEntry, ObjectMetadata, Storage};
pub use config::StorageConfig;
pub use error::{Result, StorageError};
pub use module::{StorageModule, StorageSetup};
pub use tls::StorageTls;

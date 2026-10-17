//! Object storage for nestrs.
//!
//! An injectable [`Storage`] client over the
//! [`object_store`](https://docs.rs/object_store) crate, whose backend the
//! store's URL picks — [`StorageConfig::url`], read from
//! `<PREFIX>_STORAGE__URL`:
//!
//! - `s3://<bucket>[/<prefix>]` — AWS S3, or a server speaking its API at
//!   [`StorageConfig::endpoint`]; presigning included;
//! - `memory:///[<prefix>]` — a store in this process, for development and
//!   tests, refused in a staging or production profile since a restart loses
//!   it; it signs nothing.
//!
//! No other scheme opens in this release, and storage is no port a third
//! party implements: object_store already abstracts the vendor, so a backend
//! is chosen by URL, never by a trait of this crate's.
//!
//! ## Usage
//!
//! Import [`StorageModule`] at the composition root. It owns its
//! [`StorageConfig`] (namespace `storage`, loaded from `<PREFIX>_STORAGE__*`),
//! opens the store at boot and registers [`Storage`] as an injectable
//! provider:
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_testing::HeadlessApp;
//! use futures_util::TryStreamExt;
//! use nest_rs_storage::{Storage, StorageConfig, StorageModule};
//!
//! // in a feature service:
//! #[injectable]
//! struct UploadsService {
//!     #[inject]
//!     storage: Arc<Storage>,
//! }
//!
//! #[module(
//!     imports = [StorageModule::for_root(StorageConfig {
//!         url: "memory:///uploads".into(),
//!         ..StorageConfig::default()
//!     })],
//!     providers = [UploadsService],
//! )]
//! struct UploadsModule;
//! # #[nest_rs_core::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let app = HeadlessApp::builder().module::<UploadsModule>().build_headless().await?;
//! # let UploadsService { storage } = &*app.container().get::<UploadsService>().expect("provided");
//!
//! storage.put_bytes("abc.txt", "hello", "text/plain").await?;
//! let keys: Vec<String> = storage.list("").map_ok(|entry| entry.key).try_collect().await?;
//! assert_eq!(keys, ["abc.txt"]);
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
//! ## At boot
//!
//! The store is proved before any provider is built: an `s3://` bucket answers
//! one page of its listing under the URL's prefix within
//! [`StorageConfig::operation_timeout`]. A bucket that does not exist, a
//! certificate the client does not accept, or a status every attempt would
//! meet (a redirect, a `400`) refuses the boot at once; a store that refuses
//! to list — its credentials, or a policy granting no `s3:ListBucket` — boots
//! with one `warn`; anything else — no answer, a `5xx`, a `408` or a `429` —
//! is retried within the budget, then refused as unreachable.
//!
//! ## How long a call waits
//!
//! A call waits for the store's answer within
//! [`StorageConfig::operation_timeout`], every retry included, and past it
//! fails naming the budget; [`put_stream`](Storage::put_stream) ships parts
//! that each fit it. A download's body is never cut for its size: one that
//! sends nothing for [`StorageConfig::read_timeout`] is resumed from where it
//! stopped, and fails naming the bound when the resumed body sends nothing
//! within it. The boot refuses an operation timeout at or past a net reaching
//! the client.
//!
//! ## What an error says
//!
//! A [`StorageError`] names the store and the operation, never a key, and
//! keeps object_store's error as its source; its chain, through
//! [`error_message`](fn@nest_rs_core::error_message), says object_store's links
//! by their kind, without the key or the URL their own sentences quote.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — Object storage operations.
pub const TARGET: &str = "nest_rs::storage";

mod backend;
mod client;
mod config;
mod error;
mod module;
mod store;
mod transfer;

pub use client::{ObjectEntry, ObjectMetadata, Storage};
pub use config::StorageConfig;
pub use error::{Result, StorageError};
pub use module::{StorageModule, StorageSetup};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    // An integration suite is a crate of its own: it cannot see `pub(crate)`.
    pub use crate::client::MULTIPART_PART_SIZE;
}

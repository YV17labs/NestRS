//! Typed errors for the [`Storage`](crate::Storage) client.
//!
//! Framework crates surface `thiserror` enums, not `anyhow` — a consumer can
//! match on the failed operation and keep the underlying `object_store` error
//! as the `source`.

use thiserror::Error;

/// A storage operation failure, tagged by the operation that produced it.
#[derive(Debug, Error)]
pub enum StorageError {
    /// The S3 client could not be built from the configured values.
    #[error("failed to initialize storage client")]
    Init(#[source] object_store::Error),
    /// Signing a presigned URL failed (the variant carries the HTTP method).
    #[error("failed to presign {method} URL")]
    Presign {
        method: String,
        #[source]
        source: object_store::Error,
    },
    /// Reading an object's metadata (`head`) failed.
    #[error("failed to read object metadata")]
    Head(#[source] object_store::Error),
    /// Downloading an object's bytes failed.
    #[error("failed to download object")]
    Get(#[source] object_store::Error),
    /// Uploading an object's bytes failed.
    #[error("failed to upload object")]
    Put(#[source] object_store::Error),
}

/// Result alias for storage operations.
pub type Result<T> = std::result::Result<T, StorageError>;

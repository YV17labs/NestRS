//! Typed errors for the [`Storage`](crate::Storage) client.

use thiserror::Error;

/// A storage operation failure, tagged by the operation that produced it.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StorageError {
    /// The S3 client could not be built from the configured values.
    #[error("failed to initialize storage client")]
    Init(#[source] object_store::Error),
    /// Signing a presigned URL failed (the variant carries the HTTP method).
    #[error("failed to presign {method} URL")]
    Presign {
        /// The HTTP method the URL was being signed for (`GET`/`PUT`).
        method: String,
        /// The underlying signing failure.
        #[source]
        source: object_store::Error,
    },
    /// Reading an object's metadata (`head`) failed.
    #[error("failed to read object metadata")]
    Head(#[source] object_store::Error),
    /// Downloading an object's bytes failed.
    #[error("failed to download object")]
    Get(#[source] object_store::Error),
    /// Listing the objects under a prefix failed.
    #[error("failed to list objects")]
    List(#[source] object_store::Error),
    /// Uploading an object's bytes failed.
    #[error("failed to upload object")]
    Put(#[source] object_store::Error),
    /// The stream feeding [`put_stream`](crate::Storage::put_stream) yielded an
    /// error, so the upload was aborted with nothing written.
    #[error("the upload source stream failed")]
    PutSource(#[source] std::io::Error),
    /// Deleting an object failed.
    #[error("failed to delete object")]
    Delete(#[source] object_store::Error),
    /// A plaintext endpoint was reached with plain HTTP disallowed — only
    /// through a hand-built [`Storage::new`](crate::Storage::new), since
    /// `StorageConfig` refuses the pairing at boot.
    #[error(
        "refusing to address {endpoint} over plain HTTP: \
         {allow_http_var} is false, so credentials must not travel unencrypted",
        allow_http_var = ::nest_rs_config::var_name("storage", "ALLOW_HTTP"),
    )]
    PlaintextEndpoint {
        /// The offending endpoint.
        endpoint: String,
    },
}

/// Lets a `get_stream` chunk feed `poem::Body::from_bytes_stream` directly,
/// which requires `E: Into<std::io::Error>`.
impl From<StorageError> for std::io::Error {
    fn from(error: StorageError) -> Self {
        std::io::Error::other(error)
    }
}

/// Result alias for storage operations.
pub type Result<T> = std::result::Result<T, StorageError>;

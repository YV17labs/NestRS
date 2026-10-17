//! Typed errors for the [`Storage`](crate::Storage) client, and how
//! [`error_message`](fn@nest_rs_core::error_message) says object_store's own
//! without the key or the URL its sentences quote.

use std::error::Error;
use std::time::Duration;

use http::Method;
use thiserror::Error;

/// An error object_store or the configuration handed over, boxed so its
/// library's release is never this crate's API: a caller reads it through
/// [`source`](Error::source) and `downcast_ref`.
type Source = Box<dyn Error + Send + Sync>;

/// A storage failure, tagged by what produced it. Each sentence names the
/// store — its URL's scheme, bucket and prefix, and an S3 endpoint, shown
/// without credentials — and never a key.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StorageError {
    /// The configuration names no store this client can open: refused before
    /// anything is sent, naming the variables to change.
    #[error("the object store cannot be opened as configured")]
    Init(#[source] Source),
    /// A plaintext endpoint was reached with plain HTTP disallowed — only
    /// through a config built in code, since `StorageConfig` refuses the
    /// pairing at boot.
    #[error(
        "refusing to address {endpoint} over plain HTTP: \
         {allow_http_var} is false, so credentials must not travel unencrypted",
        allow_http_var = ::nest_rs_config::var_name("storage", "ALLOW_HTTP"),
    )]
    PlaintextEndpoint {
        /// The offending endpoint, as
        /// [`endpoint_shown`](nest_rs_config::endpoint_shown) shows it: no
        /// userinfo, query or fragment.
        endpoint: String,
    },
    /// Opening the store met what every attempt would: a bucket that does not
    /// exist, a certificate the client does not accept.
    #[error("the object store {store} cannot be opened: {reason}")]
    Refused {
        /// The store, as its sentences show it.
        store: String,
        /// What to change, naming the variables.
        reason: String,
        /// What the store answered.
        #[source]
        source: Source,
    },
    /// The store did not answer the boot's proof within the operation budget.
    #[error("the object store {store} was not reached within {budget:?}")]
    Unreachable {
        /// The store, as its sentences show it.
        store: String,
        /// The budget the proof was held to.
        budget: Duration,
        /// The last attempt's failure, if one ended before the budget did.
        #[source]
        source: Option<Source>,
    },
    /// The backend cannot do what was asked: presigning on `memory`.
    #[error("the object store's {backend} backend cannot {operation}")]
    Unsupported {
        /// What was asked (`"presign"`).
        operation: &'static str,
        /// The backend that cannot (`"memory"`).
        backend: &'static str,
    },
    /// A presign's argument holds what no signature binds: refused before
    /// anything is signed, naming the argument and never its value.
    #[error("refusing to presign a URL: `{argument}` {reason}")]
    Unsignable {
        /// The argument refused (`"SignedPut::content_type"`, `"expires"`).
        argument: &'static str,
        /// What a signature takes in its place.
        reason: &'static str,
    },
    /// Signing a presigned URL failed.
    #[error("failed to presign a {method} URL for the object store {store}")]
    Presign {
        /// The method the URL was being signed for.
        method: Method,
        /// The store, as its sentences show it.
        store: String,
        /// The underlying signing failure.
        #[source]
        source: Source,
    },
    /// Reading an object's metadata (`head`) failed.
    #[error("failed to read an object's metadata from the object store {store}")]
    Head {
        /// The store, as its sentences show it.
        store: String,
        /// What the store answered.
        #[source]
        source: Source,
    },
    /// Downloading an object's bytes failed.
    #[error("failed to download an object from the object store {store}")]
    Get {
        /// The store, as its sentences show it.
        store: String,
        /// What the store answered.
        #[source]
        source: Source,
    },
    /// Listing the objects under a prefix failed.
    #[error("failed to list the objects of the object store {store}")]
    List {
        /// The store, as its sentences show it.
        store: String,
        /// What the store answered.
        #[source]
        source: Source,
    },
    /// Uploading an object's bytes failed.
    #[error("failed to upload an object to the object store {store}")]
    Put {
        /// The store, as its sentences show it.
        store: String,
        /// What the store answered.
        #[source]
        source: Source,
    },
    /// The stream feeding [`put_stream`](crate::Storage::put_stream) yielded an
    /// error, so the upload was aborted with nothing written.
    #[error("the upload source stream failed")]
    PutSource(#[source] std::io::Error),
    /// Deleting an object failed.
    #[error("failed to delete an object from the object store {store}")]
    Delete {
        /// The store, as its sentences show it.
        store: String,
        /// What the store answered.
        #[source]
        source: Source,
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

/// How object_store's errors are said: its sentences quote the object's path,
/// its retry error the request's URI — the key, a value when keys carry user
/// input — and its HTTP client's the URL. Each link is said by its kind, and
/// what lies below by the links that quote neither.
fn say(link: &(dyn Error + 'static)) -> Option<String> {
    let error = link.downcast_ref::<object_store::Error>()?;
    let kind = match error {
        object_store::Error::Generic { store, .. } => {
            return Some(below(link.source(), (*store).to_owned()));
        }
        object_store::Error::NotFound { .. } => "no object at that key",
        object_store::Error::InvalidPath { .. } => "the key is not a valid path",
        object_store::Error::JoinError { .. } => "the store's task did not finish",
        object_store::Error::NotSupported { .. } => "the store does not support the operation",
        object_store::Error::AlreadyExists { .. } => "an object already exists at that key",
        object_store::Error::Precondition { .. } => "the object's version is not the one asked for",
        object_store::Error::NotModified { .. } => "the object was not modified",
        object_store::Error::NotImplemented { .. } => "the store does not implement the operation",
        object_store::Error::PermissionDenied { .. } => {
            "the store refused the credentials permission"
        }
        object_store::Error::Unauthenticated { .. } => "the store did not accept the credentials",
        object_store::Error::UnknownConfigurationKey { .. } => "an unknown configuration key",
        _ => "the store failed",
    };
    Some(below(link.source(), kind.to_owned()))
}

nest_rs_core::inventory::submit! { nest_rs_core::SafeLink::new(say) }

/// `said`, followed by what the links from `link` down say without quoting a
/// URL: an I/O error (a refused connection, a TLS certificate, the client's
/// own budget), the HTTP client's kind of failure, and what the store answered
/// — never its body, which may name the key.
fn below(mut link: Option<&(dyn Error + 'static)>, said: String) -> String {
    let mut sentence = said;
    while let Some(current) = link {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            sentence.push_str(": ");
            sentence.push_str(&io.to_string());
            return sentence;
        }
        let part = if let Some(http) = current.downcast_ref::<object_store::client::HttpError>() {
            Some(http_kind(http.kind()).to_owned())
        } else {
            answered(&current.to_string())
        };
        if let Some(part) = part {
            sentence.push_str(": ");
            sentence.push_str(&part);
        }
        link = current.source();
    }
    sentence
}

/// What an HTTP failure of `kind` was, as a link says it.
fn http_kind(kind: object_store::client::HttpErrorKind) -> &'static str {
    use object_store::client::HttpErrorKind;
    match kind {
        HttpErrorKind::Connect => "could not connect",
        HttpErrorKind::Request => "the request failed",
        HttpErrorKind::Timeout => "the request timed out",
        HttpErrorKind::Interrupted => "the answer was interrupted",
        HttpErrorKind::Decode => "the answer could not be decoded",
        _ => "the HTTP exchange failed",
    }
}

/// What the store answered, read off object_store's sentence for a failed
/// request, whose type (`client::retry::RequestError`) is not reachable: its
/// constant part, and a status without the body that follows it.
fn answered(sentence: &str) -> Option<String> {
    if let Some(rest) = sentence.strip_prefix("Server returned non-2xx status code: ") {
        let code = rest.split(':').next().unwrap_or_default().trim();
        return Some(format!("the store answered {code}"));
    }
    if sentence.starts_with("Server returned error response: ") {
        return Some("the store answered an error inside a successful answer".to_owned());
    }
    sentence
        .starts_with("Received redirect without LOCATION")
        .then(|| "the store redirected without a location, the sign of a wrong region".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_keeps_its_source_reachable_by_its_type() {
        let refused = StorageError::Get {
            store: "s3://media".into(),
            source: Box::new(object_store::Error::NotFound {
                path: "users/ada@example.com/avatar.png".into(),
                source: "absent".into(),
            }),
        };
        let source = refused.source().expect("the store's error is the source");
        assert!(
            source.downcast_ref::<object_store::Error>().is_some(),
            "{source:?}"
        );
    }

    #[test]
    fn a_chain_says_the_store_and_what_it_answered_never_the_key() {
        let key = "users/ada@example.com/avatar.png";
        let refused = StorageError::Get {
            store: "s3://media".into(),
            source: Box::new(object_store::Error::NotFound {
                path: key.into(),
                source: "absent".into(),
            }),
        };
        let chain = nest_rs_core::error_message(&refused);
        assert_eq!(
            chain,
            "failed to download an object from the object store s3://media: no object at that key"
        );
        assert!(!chain.contains("ada@example.com"), "{chain}");
    }

    #[test]
    fn a_generic_failure_says_its_store_and_the_io_error_below() {
        let refused = StorageError::Put {
            store: "s3://media".into(),
            source: Box::new(object_store::Error::Generic {
                store: "S3",
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the call did not end within 15s",
                )),
            }),
        };
        assert_eq!(
            nest_rs_core::error_message(&refused),
            "failed to upload an object to the object store s3://media: S3: the call did not end \
             within 15s"
        );
    }

    #[test]
    fn a_status_is_read_without_the_body_that_follows() {
        assert_eq!(
            answered("Server returned non-2xx status code: 404 Not Found: <Key>ada</Key>")
                .as_deref(),
            Some("the store answered 404 Not Found")
        );
        assert_eq!(answered("Error performing GET https://s3/bucket/ada"), None);
    }

    #[test]
    fn what_the_store_answered_is_said_by_its_constant_part_never_its_body() {
        assert_eq!(
            answered(
                "Received redirect without LOCATION, this normally indicates an incorrectly \
                 configured region"
            )
            .as_deref(),
            Some("the store redirected without a location, the sign of a wrong region")
        );
        let said = answered("Server returned error response: <Key>ada</Key><Code>SlowDown</Code>")
            .expect("an error inside a successful answer is said");
        assert!(!said.contains("ada"), "{said}");
    }
}

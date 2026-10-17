//! The sentences every transport tells a client in place of the server's own
//! text: `<CONDITION>_CLIENT_MESSAGE`, one per condition a `5xx` names.
//!
//! Each client-facing transport has its own `Opaque` trait, whose `opaque()` logs
//! the real error and substitutes [`OPAQUE_CLIENT_MESSAGE`]: one trait per
//! transport lets `.opaque()?` infer its output from the enclosing return type.

/// What a client is told when an operation fails for a reason it must not read
/// (a `500`).
///
/// Constant on purpose: a message assembled from the error would leak by
/// construction.
pub const OPAQUE_CLIENT_MESSAGE: &str = "internal error";

/// What a client is told when something the server depends on did not answer
/// (a `503`): RFC 9110 §15.6.4's reason phrase, lowercased as
/// [`OPAQUE_CLIENT_MESSAGE`] is. Whatever the refusal said of the dependency
/// stays on the server.
pub const UNAVAILABLE_CLIENT_MESSAGE: &str = "service unavailable";

/// Re-exported through the crate's `__private`.
pub(crate) mod __private {
    use std::borrow::Cow;
    use std::error::Error;
    use std::fmt;
    use std::sync::Arc;

    /// What an opaque failure keeps for the operator, as the link of its chain
    /// below the constant it displays: the detail as its `Display`, the cause as
    /// its [`source`](Error::source).
    ///
    /// [`error_message`](fn@crate::error_message) reads through it; an edge looking
    /// for what to answer stops at it, so nothing beneath — a decode failure, a
    /// `Problem` — is answered as the client's.
    #[derive(Clone, Debug)]
    pub struct OpaqueCause {
        detail: Cow<'static, str>,
        source: Option<Arc<dyn Error + Send + Sync>>,
    }

    impl OpaqueCause {
        /// The link saying `detail`, then `source`'s chain.
        pub fn new(
            detail: impl Into<Cow<'static, str>>,
            source: Option<Arc<dyn Error + Send + Sync>>,
        ) -> Self {
            Self {
                detail: detail.into(),
                source,
            }
        }
    }

    impl fmt::Display for OpaqueCause {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.detail)
        }
    }

    impl Error for OpaqueCause {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source
                .as_deref()
                .map(|source| source as &(dyn Error + 'static))
        }
    }
}

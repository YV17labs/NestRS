//! The sentences every transport tells a client in place of the server's own
//! text: `<CONDITION>_CLIENT_MESSAGE`, one per condition a `5xx` names.
//!
//! Each client-facing transport has its own `Opaque` trait, whose `opaque()`
//! substitutes [`OPAQUE_CLIENT_MESSAGE`] (on HTTP, the `500` problem document
//! under the `INTERNAL` code, its `title` saying as much) and keeps the real error
//! for the operator: HTTP and GraphQL file it at the call, WebSocket and MCP hand
//! it to the edge, which files it naming the unit once that unit is answered. One
//! trait per transport lets `.opaque()?` infer its output from the enclosing
//! return type.

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
    /// `Problem` — is answered as the client's. Every other reader reads what
    /// `error_message` says too: its `Debug` is that sentence, and its `source()`
    /// the cause as a [`SaidFailure`](crate::__private::SaidFailure), so a `{:?}`
    /// or a `main` returning the failure prints no value the cause quoted.
    #[derive(Clone)]
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

        /// [`new`](Self::new) over a failure's `source`, every link kept as the
        /// type it is ([`boxed_error`](fn@crate::boxed_error)), so a driver's
        /// sayer still claims its own.
        pub fn because(
            detail: impl Into<Cow<'static, str>>,
            source: impl Into<Box<dyn Error + Send + Sync>> + 'static,
        ) -> Self {
            Self::new(detail, Some(Arc::from(crate::boxed_error(source))))
        }
    }

    /// The detail, said without what a decode failure or a driver's error in the
    /// cause quotes wherever the detail spells it.
    impl fmt::Display for OpaqueCause {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let cause = self
                .source
                .as_deref()
                .map(|cause| cause as &(dyn Error + 'static));
            f.write_str(&crate::DecodeError::redact(&self.detail, cause))
        }
    }

    impl fmt::Debug for OpaqueCause {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            fmt::Debug::fmt(&crate::error_message(self), f)
        }
    }

    impl Error for OpaqueCause {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source
                .as_ref()
                .map(|cause| crate::__private::SaidFailure::of(cause) as &(dyn Error + 'static))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::__private::OpaqueCause;
    use crate::error_message;

    fn refused() -> serde_json::Error {
        serde_json::from_str::<u64>(r#""zzqx-token""#).expect_err("not a number")
    }

    /// A `{:?}`, an `anyhow` report and a `main` returning it read what
    /// `error_message` says of the cause, and nothing it quoted.
    #[test]
    fn no_reader_of_an_opaque_cause_prints_what_its_cause_quotes() {
        for cause in [
            OpaqueCause::because("the provider's reply did not decode", refused()),
            OpaqueCause::because(
                "the provider's reply did not decode",
                anyhow::Error::from(refused()).context("reading the reply"),
            ),
            OpaqueCause::because(
                format!("the reply did not decode: {}", refused()),
                refused(),
            ),
        ] {
            let said = error_message(&cause);
            assert!(
                said.contains("invalid type: a string, expected u64"),
                "{said}"
            );
            let printed = nest_rs_testing::printed(&cause);
            assert!(!printed.contains("zzqx"), "{printed}");
            let report = format!("{:?}", anyhow::Error::new(cause));
            assert!(!report.contains("zzqx"), "{report}");
        }
    }

    /// What the cause said before is what every reader says now, its chain
    /// read once and no link below it handed out.
    #[test]
    fn an_opaque_cause_says_its_detail_then_its_cause_once() {
        let cause = OpaqueCause::because(
            "the provider's reply did not decode",
            anyhow::Error::from(refused()).context("reading the reply"),
        );
        let said = "the provider's reply did not decode: reading the reply: invalid type: a \
                    string, expected u64 at line 1 column 12";
        assert_eq!(error_message(&cause), said);
        assert_eq!(format!("{cause:?}"), format!("{said:?}"));
        let below = cause.source().expect("the cause, said");
        assert!(below.source().is_none(), "nothing beneath the said cause");
        assert_eq!(cause.to_string(), "the provider's reply did not decode");
        assert_eq!(
            error_message(&OpaqueCause::new("the provider answered 400", None)),
            "the provider answered 400"
        );
    }
}

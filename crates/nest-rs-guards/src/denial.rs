//! [`Denial`] — transport-agnostic guard rejection.
//!
//! A guard returns `Err(Denial::...)`; each edge renders it in its native
//! error form.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// RFC 6750 §3.1's code for a token too narrow for the operation — the one
/// `nest_rs_http::challenge::INSUFFICIENT_SCOPE` writes on the challenge.
const INSUFFICIENT_SCOPE: &str = "insufficient_scope";

/// What a [`Guard`](crate::Guard) returns on rejection.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Denial {
    /// 401 — authentication missing or invalid.
    Unauthorized(Cow<'static, str>),

    /// 401 — a credential was **presented and rejected**, carrying the RFC 6750
    /// §3.1 code that says why.
    ///
    /// A request with no credential is told no reason (RFC 6750 §3), so a client
    /// can tell "refresh and retry" from "start discovery".
    InvalidCredential {
        /// Human-readable reason, rendered as the problem envelope's `detail`.
        reason: Cow<'static, str>,
        /// The §3.1 code: `invalid_token` for every credential this framework
        /// rejects. `invalid_request` describes a malformed request rather than
        /// a bad credential, and `insufficient_scope` has its own variant.
        code: &'static str,
    },

    /// 403 — authentication ok but the caller may not perform this operation.
    Forbidden(Cow<'static, str>),

    /// 403 — the token is valid but too narrow: the operation is gated behind
    /// scopes the credential does not carry (RFC 6750 §3.1).
    ///
    /// The transports carry `required` to the edge, where the OAuth challenge
    /// naming those scopes is written.
    InsufficientScope {
        /// Scopes that would have granted the operation. Empty is legal — a
        /// deployment may refuse without naming its internals — and then this
        /// renders exactly like a plain `403`.
        required: Vec<String>,
        /// Human-readable reason for the denial.
        reason: Cow<'static, str>,
    },

    /// 429 — rate limit exceeded.
    RateLimited {
        /// Seconds until the caller may retry (the `Retry-After` value).
        retry_after_secs: u32,
        /// Human-readable reason for the denial.
        reason: Cow<'static, str>,
    },

    /// 503 — the request could not be evaluated because something the guard
    /// depends on did not answer: an identity store, an identity provider. The
    /// caller did nothing wrong, so it is no `401` (RFC 9110 §15.6.4), and it
    /// is told when to come back when that is known.
    ///
    /// Built by [`unavailable`](Denial::unavailable) or, with what failed,
    /// [`unavailable_because`](Denial::unavailable_because).
    #[non_exhaustive]
    Unavailable {
        /// Seconds until a retry may succeed (the `Retry-After` value), when
        /// the failing party said so.
        retry_after_secs: Option<u32>,
        /// Human-readable reason for the denial.
        reason: Cow<'static, str>,
        /// What failed, read through [`Error::source`]: its chain is the
        /// operator's, never the client's.
        source: Option<Arc<dyn Error + Send + Sync>>,
    },

    /// 500 — a wiring bug surfaced at request time (e.g. an authz guard ran
    /// before any authn guard attached an identity). Not a security event.
    Internal(Cow<'static, str>),
}

impl Denial {
    /// A `401 Unauthorized` denial — authentication is missing or invalid.
    pub fn unauthorized(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::Unauthorized(reason.into())
    }

    /// A `403 Forbidden` denial — authenticated but not permitted.
    pub fn forbidden(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::Forbidden(reason.into())
    }

    /// A `403 Forbidden` denial for a token too narrow to authorize the
    /// operation — `required` names the scopes that would have granted it, and
    /// reaches the client as the RFC 6750 `insufficient_scope` challenge.
    pub fn insufficient_scope(
        required: impl IntoIterator<Item = impl Into<String>>,
        reason: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::InsufficientScope {
            required: required.into_iter().map(Into::into).collect(),
            reason: reason.into(),
        }
    }

    /// A `429 Too Many Requests` denial with the `Retry-After` hint.
    pub fn rate_limited(retry_after_secs: u32, reason: impl Into<Cow<'static, str>>) -> Self {
        Self::RateLimited {
            retry_after_secs,
            reason: reason.into(),
        }
    }

    /// A `503 Service Unavailable` denial — something the guard depends on did
    /// not answer — with the `Retry-After` hint when it is known.
    pub fn unavailable(
        retry_after_secs: Option<u32>,
        reason: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::Unavailable {
            retry_after_secs,
            reason: reason.into(),
            source: None,
        }
    }

    /// [`unavailable`](Self::unavailable), keeping what failed as the denial's
    /// [`source`](Error::source). A source never quotes a value the caller sent:
    /// its chain reaches a log line.
    pub fn unavailable_because(
        retry_after_secs: Option<u32>,
        reason: impl Into<Cow<'static, str>>,
        source: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self::Unavailable {
            retry_after_secs,
            reason: reason.into(),
            source: Some(Arc::from(source.into())),
        }
    }

    /// The `Retry-After` value this denial carries, in seconds: a rate limit's
    /// always, an unavailable dependency's when it is known, nothing else's.
    pub fn retry_after_secs(&self) -> Option<u32> {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => Some(*retry_after_secs),
            Self::Unavailable {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }

    /// A `500` denial — a guard wiring bug surfaced at request time.
    pub fn internal(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::Internal(reason.into())
    }

    /// A credential was presented and rejected, with the RFC 6750 §3.1 code
    /// saying why. Use this rather than [`unauthorized`](Self::unauthorized)
    /// whenever a credential actually arrived — the two render differently on
    /// the challenge, and the difference is what a client acts on.
    pub fn invalid_credential(reason: impl Into<Cow<'static, str>>, code: &'static str) -> Self {
        Self::InvalidCredential {
            reason: reason.into(),
            code,
        }
    }

    /// The RFC 6750 §3.1 error code this denial reports on the
    /// `WWW-Authenticate` challenge, or `None` when the specification says to
    /// report none.
    pub fn bearer_error(&self) -> Option<&'static str> {
        match self {
            Self::InvalidCredential { code, .. } => Some(code),
            Self::InsufficientScope { .. } => Some(INSUFFICIENT_SCOPE),
            _ => None,
        }
    }

    /// HTTP status code analog — the value transports report.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Unauthorized(_) | Self::InvalidCredential { .. } => 401,
            Self::Forbidden(_) | Self::InsufficientScope { .. } => 403,
            Self::RateLimited { .. } => 429,
            Self::Internal(_) => 500,
            Self::Unavailable { .. } => 503,
        }
    }

    /// The scopes that would have granted the refused operation — empty for
    /// every denial that is not about scope, and for an
    /// [`InsufficientScope`](Self::InsufficientScope) that names none.
    pub fn required_scopes(&self) -> &[String] {
        match self {
            Self::InsufficientScope { required, .. } => required,
            _ => &[],
        }
    }

    /// Human-readable reason.
    pub fn message(&self) -> &str {
        match self {
            Self::Unauthorized(s) | Self::Forbidden(s) | Self::Internal(s) => s.as_ref(),
            Self::RateLimited { reason, .. }
            | Self::Unavailable { reason, .. }
            | Self::InsufficientScope { reason, .. }
            | Self::InvalidCredential { reason, .. } => reason.as_ref(),
        }
    }
}

/// What failed behind `denial`, as the `error` field of the line an edge files
/// for it: the whole chain, its reason first, when the denial keeps a cause, and
/// nothing otherwise. The client never reads it.
pub fn failure_chain(denial: &Denial) -> Option<String> {
    Error::source(denial).map(|_| nest_rs_core::error_message(denial))
}

/// The denial a class gate's `refused` carries, for the edge to render as it
/// renders a guard's; an opaque one, filed at `error`, for a gate that failed
/// with anything else — a gate's failure is a refusal, never a pass.
pub fn denial_of(refused: &nest_rs_core::Failure) -> Denial {
    refused.find::<Denial>().cloned().unwrap_or_else(|| {
        tracing::error!(
            target: nest_rs_core::target::LAYERS,
            error = %nest_rs_core::error_message(refused.error()),
            "a class gate refused with no denial",
        );
        Denial::internal("a class gate refused with no denial")
    })
}

/// The reason, as a log line or a failure chain reads it.
impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl Error for Denial {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable {
                source: Some(source),
                ..
            } => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct StoreDown;

    impl fmt::Display for StoreDown {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the identity store refused the connection")
        }
    }

    impl Error for StoreDown {}

    #[test]
    fn an_unavailable_denial_keeps_what_failed_as_its_source() {
        let denial = Denial::unavailable_because(Some(30), "the identity store is down", StoreDown);
        assert_eq!(denial.http_status(), 503);
        assert_eq!(denial.retry_after_secs(), Some(30));
        assert_eq!(denial.message(), "the identity store is down");
        assert_eq!(
            nest_rs_core::error_message(&denial),
            "the identity store is down: the identity store refused the connection",
        );
        assert!(
            Denial::unavailable(None, "down").source().is_none(),
            "a denial built without a cause has none",
        );
    }

    #[test]
    fn only_a_denial_keeping_its_cause_has_a_chain_to_file() {
        let sourced = Denial::unavailable_because(None, "the identity store is down", StoreDown);
        assert_eq!(
            failure_chain(&sourced).as_deref(),
            Some("the identity store is down: the identity store refused the connection"),
        );
        assert_eq!(failure_chain(&Denial::unavailable(None, "down")), None);
        assert_eq!(failure_chain(&Denial::forbidden("no")), None);
    }

    #[test]
    fn a_scope_denial_names_rfc_6750s_code() {
        let denial = Denial::insufficient_scope(["posts:write"], "a narrower token");
        assert_eq!(denial.bearer_error(), Some("insufficient_scope"));
        assert_eq!(denial.required_scopes(), ["posts:write".to_owned()]);
    }
}

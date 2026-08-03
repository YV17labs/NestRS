//! [`Denial`] — transport-agnostic guard rejection.
//!
//! A guard returns `Err(Denial::...)`; each transport's shaper converts it
//! to that transport's native error (HTTP `Response`, GraphQL error frame,
//! WS error message). The dev never reaches for a transport-specific error
//! type.

use std::borrow::Cow;

/// What a [`Guard`](crate::Guard) returns on rejection.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Denial {
    /// 401 — authentication missing or invalid.
    Unauthorized(Cow<'static, str>),

    /// 403 — authentication ok but the caller may not perform this operation.
    Forbidden(Cow<'static, str>),

    /// 403 — the token is valid but too narrow: the operation is gated behind
    /// scopes the credential does not carry (RFC 6750 §3.1).
    ///
    /// Distinct from [`Forbidden`](Self::Forbidden) because the remedy is
    /// different, and only the client can apply it: a plain `403` says "you may
    /// not", this says "come back with a wider token, here is which one". The
    /// transports carry `required` to the edge so the OAuth challenge naming
    /// those scopes is written in exactly one place.
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

    /// A `500` denial — a guard wiring bug surfaced at request time.
    pub fn internal(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::Internal(reason.into())
    }

    /// HTTP status code analog — the value transports report.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) | Self::InsufficientScope { .. } => 403,
            Self::RateLimited { .. } => 429,
            Self::Internal(_) => 500,
        }
    }

    /// The scopes that would have granted the refused operation — empty for
    /// every denial that is not about scope, and for an
    /// [`InsufficientScope`](Self::InsufficientScope) that names none.
    ///
    /// One accessor rather than a `match` per transport: "an empty set carries
    /// no challenge" is the rule each renderer would otherwise re-encode, and a
    /// transport added later gets it for free.
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
            Self::RateLimited { reason, .. } | Self::InsufficientScope { reason, .. } => {
                reason.as_ref()
            }
        }
    }
}

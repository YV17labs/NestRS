//! [`Denial`] — transport-agnostic guard rejection.
//!
//! A guard returns `Err(Denial::...)`; each transport's shaper converts it
//! to that transport's native error (HTTP `Response`, GraphQL error frame,
//! WS error message). The dev never reaches for a transport-specific error
//! type.

use std::borrow::Cow;

/// What a [`Guard`](crate::Guard) returns on rejection.
#[derive(Clone, Debug)]
pub enum Denial {
    /// 401 — authentication missing or invalid.
    Unauthorized(Cow<'static, str>),

    /// 403 — authentication ok but the caller may not perform this operation.
    Forbidden(Cow<'static, str>),

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
            Self::Forbidden(_) => 403,
            Self::RateLimited { .. } => 429,
            Self::Internal(_) => 500,
        }
    }

    /// Human-readable reason.
    pub fn message(&self) -> &str {
        match self {
            Self::Unauthorized(s) | Self::Forbidden(s) | Self::Internal(s) => s.as_ref(),
            Self::RateLimited { reason, .. } => reason.as_ref(),
        }
    }
}

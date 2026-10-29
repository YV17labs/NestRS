//! [`Denial`] — transport-agnostic guard rejection.
//!
//! A guard returns `Err(Denial::...)`; each edge renders it in its native
//! error form.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use nest_rs_core::MissingAmbientError;

/// RFC 6750 §3.1's code for a token too narrow for the operation — the one
/// `nest_rs_http::challenge::INSUFFICIENT_SCOPE` writes on the challenge.
const INSUFFICIENT_SCOPE: &str = "insufficient_scope";

/// What a [`Guard`](crate::Guard) returns on rejection: a class, the sentence
/// the client reads, and what the edge files beside it — the
/// [`reason`](Self::reason) a guard names and the [`source`](Error::source) of
/// a refusal something failed behind.
///
/// Built by its class's constructor, then the structured fields:
///
/// ```
/// use nest_rs_guards::Denial;
///
/// let denial = Denial::forbidden("this author may not publish").with_reason("banned_author");
/// assert_eq!(denial.http_status(), 403);
/// assert_eq!(denial.reason(), Some("banned_author"));
/// ```
#[derive(Clone, Debug)]
pub struct Denial {
    class: Class,
    message: Cow<'static, str>,
    reason: Option<&'static str>,
    source: Option<Arc<dyn Error + Send + Sync>>,
}

/// A denial's class, and what that class carries.
#[derive(Clone, Debug)]
pub(crate) enum Class {
    /// 401 — authentication missing or invalid.
    Unauthorized,
    /// 401 — a credential was presented and rejected, with the RFC 6750 §3.1
    /// code saying why.
    InvalidCredential { code: &'static str },
    /// 403 — the caller may not perform this operation.
    Forbidden,
    /// 403 — the token is valid but too narrow (RFC 6750 §3.1).
    InsufficientScope { required: Vec<String> },
    /// 429 — rate limit exceeded.
    RateLimited { retry_after_secs: u32 },
    /// 503 — something the guard depends on did not answer.
    Unavailable { retry_after_secs: Option<u32> },
    /// 500 — a wiring defect surfaced at request time.
    Internal,
}

impl Denial {
    fn of(class: Class, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            class,
            message: message.into(),
            reason: None,
            source: None,
        }
    }

    /// A `401 Unauthorized` denial — authentication is missing or invalid.
    pub fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        Self::of(Class::Unauthorized, message)
    }

    /// A `401` for a credential that was **presented and rejected**, with the
    /// RFC 6750 §3.1 code saying why — `invalid_token` for every credential this
    /// framework rejects. Use it rather than [`unauthorized`](Self::unauthorized)
    /// whenever a credential actually arrived: a request with no credential is
    /// told no reason (RFC 6750 §3), so a client can tell "refresh and retry"
    /// from "start discovery".
    pub fn invalid_credential(message: impl Into<Cow<'static, str>>, code: &'static str) -> Self {
        Self::of(Class::InvalidCredential { code }, message)
    }

    /// A `403 Forbidden` denial — authenticated but not permitted.
    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::of(Class::Forbidden, message)
    }

    /// A `403 Forbidden` denial for a token too narrow to authorize the
    /// operation — `required` names the scopes the request needs, every one of
    /// them (RFC 6750 §3's `scope`), and reaches the client as the
    /// `insufficient_scope` challenge. Empty is legal — a deployment may refuse
    /// without naming its internals — and then it renders as a plain `403`.
    pub fn insufficient_scope(
        required: impl IntoIterator<Item = impl Into<String>>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        let required = required.into_iter().map(Into::into).collect();
        Self::of(Class::InsufficientScope { required }, message)
    }

    /// A `429 Too Many Requests` denial with the `Retry-After` hint.
    pub fn rate_limited(retry_after_secs: u32, message: impl Into<Cow<'static, str>>) -> Self {
        Self::of(Class::RateLimited { retry_after_secs }, message)
    }

    /// A `503 Service Unavailable` denial — the request could not be evaluated
    /// because something the guard depends on did not answer, an identity
    /// store or provider: the caller did nothing wrong, so it is no `401`
    /// (RFC 9110 §15.6.4) — with the `Retry-After` hint when it is known. What
    /// failed goes in [`with_source`](Self::with_source).
    pub fn unavailable(
        retry_after_secs: Option<u32>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::of(Class::Unavailable { retry_after_secs }, message)
    }

    /// A `500` denial — a wiring defect surfaced at request time (an
    /// authorization guard ran before any authentication guard attached an
    /// identity). Its message is the server's own: the client reads only its
    /// class.
    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::of(Class::Internal, message)
    }

    /// Name why this guard refused, in lowercase snake case (`banned_author`,
    /// `not_a_user`): the `reason` field of the line the edge files for the
    /// denial. An operator's word, never the client's: no edge answers it.
    #[must_use]
    pub fn with_reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(reason);
        self
    }

    /// Keep what failed behind this denial as its [`source`](Error::source),
    /// every link as the type it is ([`boxed_error`](nest_rs_core::boxed_error)),
    /// so a driver's own sayer still claims its link. The edge files its chain;
    /// the client never reads it. A source never quotes a value the caller
    /// sent: its chain reaches a log line.
    #[must_use]
    pub fn with_source(
        mut self,
        source: impl Into<Box<dyn Error + Send + Sync>> + 'static,
    ) -> Self {
        self.source = Some(Arc::from(nest_rs_core::boxed_error(source)));
        self
    }

    /// Why the guard refused, as it named it with
    /// [`with_reason`](Self::with_reason).
    pub fn reason(&self) -> Option<&'static str> {
        self.reason
    }

    /// The `Retry-After` value this denial carries, in seconds: a rate limit's
    /// always, an unavailable dependency's when it is known, nothing else's.
    pub fn retry_after_secs(&self) -> Option<u32> {
        match self.class {
            Class::RateLimited { retry_after_secs } => Some(retry_after_secs),
            Class::Unavailable { retry_after_secs } => retry_after_secs,
            _ => None,
        }
    }

    /// The RFC 6750 §3.1 error code this denial reports on the
    /// `WWW-Authenticate` challenge, or `None` when the specification says to
    /// report none.
    pub fn bearer_error(&self) -> Option<&'static str> {
        match self.class {
            Class::InvalidCredential { code } => Some(code),
            Class::InsufficientScope { .. } => Some(INSUFFICIENT_SCOPE),
            _ => None,
        }
    }

    /// HTTP status code analog — the value transports report, and the class a
    /// caller tells denials apart by.
    pub fn http_status(&self) -> u16 {
        match self.class {
            Class::Unauthorized | Class::InvalidCredential { .. } => 401,
            Class::Forbidden | Class::InsufficientScope { .. } => 403,
            Class::RateLimited { .. } => 429,
            Class::Internal => 500,
            Class::Unavailable { .. } => 503,
        }
    }

    /// The scopes the refused operation needs, every one of them — empty for
    /// every denial that is not about scope, and for an insufficient-scope one
    /// that names none.
    pub fn required_scopes(&self) -> &[String] {
        match &self.class {
            Class::InsufficientScope { required } => required,
            _ => &[],
        }
    }

    /// The human-readable sentence: what a `4xx` answers, and a `5xx`'s
    /// account for the operator alone.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The denial's class, for this crate's renderings.
    pub(crate) fn class(&self) -> &Class {
        &self.class
    }
}

/// What failed behind `denial`, as the `error` field of the line an edge files
/// for it: the whole chain, its message first, when the denial keeps a cause,
/// and nothing otherwise — a guard's own text may name what no line carries.
/// The client never reads it.
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

/// The reason a denial built from a [`MissingAmbientError`] names.
pub(crate) const MISSING_AMBIENT: &str = "missing_ambient";

/// A wiring precondition a guard's `?` met
/// ([`UnitContext::require`](nest_rs_core::UnitContext::require)): a `500`,
/// filed once at `error` by the edge with the guard and the value it named.
impl From<MissingAmbientError> for Denial {
    fn from(missing: MissingAmbientError) -> Self {
        Self::internal("a value the guard requires is not ambient")
            .with_reason(MISSING_AMBIENT)
            .with_source(missing)
    }
}

/// The reason, as a log line or a failure chain reads it.
impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl Error for Denial {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
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
        let denial =
            Denial::unavailable(Some(30), "the identity store is down").with_source(StoreDown);
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

    /// A driver's error quoting a value, said without it by the driver's
    /// [`SafeLink`](nest_rs_core::SafeLink).
    #[derive(Debug)]
    struct Quoting;

    impl fmt::Display for Quoting {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("row refused: ada@example.com")
        }
    }

    impl Error for Quoting {}

    nest_rs_core::inventory::submit! {
        nest_rs_core::SafeLink::new(|link| link.is::<Quoting>().then(|| "row refused".to_owned()))
    }

    /// A cause handed over as an `anyhow::Error` keeps its links' types, so
    /// the driver's sayer still claims its own and the value stays out of the
    /// line; anyhow's own conversion boxes a wrapper whose `source` skips them.
    #[test]
    fn an_anyhow_cause_is_said_by_its_links_own_sayers() {
        let cause = nest_rs_core::anyhow::Error::new(Quoting);
        let denial = Denial::unavailable(None, "the identity store is down").with_source(cause);
        assert_eq!(
            nest_rs_core::error_message(&denial),
            "the identity store is down: row refused",
        );
    }

    #[test]
    fn a_denial_keeping_its_cause_or_the_servers_has_a_chain_to_file() {
        let sourced =
            Denial::unavailable(None, "the identity store is down").with_source(StoreDown);
        assert_eq!(
            failure_chain(&sourced).as_deref(),
            Some("the identity store is down: the identity store refused the connection"),
        );
        assert_eq!(
            failure_chain(&Denial::internal("redis at 10.0.3.7 refused")),
            None,
            "a guard's own text never reaches a line",
        );
        let refused = Denial::forbidden("no").with_source(StoreDown);
        assert_eq!(
            failure_chain(&refused).as_deref(),
            Some("no: the identity store refused the connection"),
        );
        assert_eq!(
            failure_chain(&Denial::forbidden("not yours")),
            None,
            "a 4xx's message is the client's sentence",
        );
    }

    #[test]
    fn a_denial_names_its_reason_for_the_line_alone() {
        let denial = Denial::forbidden("writing a post takes a token naming a user")
            .with_reason("not_a_user");
        assert_eq!(denial.reason(), Some("not_a_user"));
        assert_eq!(
            denial.message(),
            "writing a post takes a token naming a user"
        );
        assert_eq!(Denial::forbidden("no").reason(), None);
    }

    #[test]
    fn a_scope_denial_names_rfc_6750s_code() {
        let denial = Denial::insufficient_scope(["posts:write"], "a narrower token");
        assert_eq!(denial.bearer_error(), Some("insufficient_scope"));
        assert_eq!(denial.required_scopes(), ["posts:write".to_owned()]);
    }
}

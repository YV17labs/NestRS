//! What a client is told when a unit fails: a [`Problem`], the [`Code`] it is
//! known by, and [`ToProblem`], the one conversion a domain error implements to
//! say it on every edge.
//!
//! Each edge answers a problem in its own standard form: an RFC 9457 problem
//! document with a `code` member on HTTP (`ProblemDetails` is its document),
//! `extensions.code` on GraphQL, `data.code` in a WebSocket error frame and in
//! an MCP JSON-RPC error — beside a field map's `errors` on every edge
//! (`.claude/decisions/every-refusal-carries-its-class.md`), and the problem's
//! RFC 9457 `type` and `title` where it names them: the document's own members
//! on HTTP, `extensions.type` and `extensions.title` on GraphQL, `data.type`
//! and `data.title` on WebSocket and MCP. Every edge reads a [`ToProblem`]
//! error by its type, a route's included, so a domain error maps to the wire
//! once, with no HTTP impl beside it. An error the edge finds no answer in is
//! answered opaquely
//! ([`OPAQUE_CLIENT_MESSAGE`]; on HTTP the `500` document under the `INTERNAL`
//! code, with no `detail`) and its chain is logged once at `error`.
//!
//! ```
//! use nest_rs_core::{Code, Problem, ToProblem, code};
//!
//! const IDEMPOTENCY_KEY_REQUIRED: Code = Code::new("IDEMPOTENCY_KEY_REQUIRED");
//! const ALREADY_PAID: Problem = Problem::new(409, code::CONFLICT).with_type(
//!     "https://example.com/problems/order-already-paid",
//!     "Order already paid",
//! );
//!
//! #[derive(Debug, thiserror::Error)]
//! enum OrderError {
//!     #[error("the order is already paid")]
//!     AlreadyPaid,
//!     #[error("no idempotency key")]
//!     NoKey,
//!     #[error("the ledger refused the write")]
//!     Ledger(#[source] std::io::Error),
//! }
//!
//! impl ToProblem for OrderError {
//!     fn to_problem(&self) -> Option<Problem> {
//!         match self {
//!             Self::AlreadyPaid => Some(ALREADY_PAID.with_detail(self.to_string())),
//!             Self::NoKey => Some(Problem::new(400, IDEMPOTENCY_KEY_REQUIRED)),
//!             Self::Ledger(_) => None,
//!         }
//!     }
//! }
//!
//! let problem = OrderError::AlreadyPaid.to_problem().expect("a client error");
//! assert_eq!((problem.status(), problem.code()), (409, code::CONFLICT));
//! assert_eq!(problem.title(), Some("Order already paid"));
//! assert!(OrderError::Ledger(std::io::Error::other("disk full")).to_problem().is_none());
//! ```
//!
//! [`OPAQUE_CLIENT_MESSAGE`]: crate::OPAQUE_CLIENT_MESSAGE

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use crate::opaque::{OPAQUE_CLIENT_MESSAGE, UNAVAILABLE_CLIENT_MESSAGE};

/// A stable, machine-readable outcome class a client branches on:
/// SCREAMING_SNAKE ASCII, checked when it is declared.
///
/// An app declares its own codes as the framework declares [`code`]'s, in a
/// `const`, where a malformed one is a compile error:
///
/// ```
/// use nest_rs_core::Code;
///
/// pub const IDEMPOTENCY_KEY_REQUIRED: Code = Code::new("IDEMPOTENCY_KEY_REQUIRED");
/// assert_eq!(IDEMPOTENCY_KEY_REQUIRED.as_str(), "IDEMPOTENCY_KEY_REQUIRED");
/// ```
///
/// ```compile_fail,E0080
/// use nest_rs_core::Code;
///
/// const CONFLICT: Code = Code::new("conflict");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Code(&'static str);

impl Code {
    /// `code` as a [`Code`]: uppercase ASCII words of letters and digits, each
    /// starting with a letter, joined by single underscores.
    ///
    /// # Panics
    ///
    /// When `code` is not of that shape. Declared in a `const`, as every code
    /// is meant to be, the panic is a compile error naming the refusal.
    #[expect(
        clippy::panic,
        reason = "evaluated in a const, where a panic is the compile error naming the refused code"
    )]
    pub const fn new(code: &'static str) -> Self {
        let bytes = code.as_bytes();
        if bytes.is_empty() {
            panic!("a `Code` is not empty");
        }
        let mut i = 0;
        let mut word_start = true;
        while i < bytes.len() {
            match bytes[i] {
                b'A'..=b'Z' => word_start = false,
                b'0'..=b'9' if !word_start => {}
                b'_' if !word_start && i + 1 < bytes.len() => word_start = true,
                _ => panic!(
                    "a `Code` is SCREAMING_SNAKE ASCII: uppercase words of letters and digits, \
                     each starting with a letter, joined by single underscores"
                ),
            }
            i += 1;
        }
        Self(code)
    }

    /// The code as a client reads it.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The code in lowercase words (`NOT_FOUND` reads `not found`): the sentence
    /// a client error carries when nothing more specific was said.
    fn in_words(self) -> String {
        self.0.replace('_', " ").to_ascii_lowercase()
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// The codes the framework answers with, one per outcome class, each read off
/// the standard that names its class.
///
/// A class one HTTP status answers by its own name is that status's registered
/// reason phrase in upper snake case (the IANA HTTP Status Code Registry,
/// RFC 9110 §15.5): `FORBIDDEN`, `NOT_FOUND`, `CONFLICT` and every class from
/// `METHOD_NOT_ALLOWED` to `UPGRADE_REQUIRED` below, and the next status the
/// framework comes to answer. A class the reason phrases do not name is read
/// off the standard that does: gRPC's canonical status names for a class
/// several statuses answer, or one whose phrase misnames it —
/// `INVALID_ARGUMENT` (a `400` or a `422`), `UNAUTHENTICATED` (a `401`,
/// whose phrase says *Unauthorized*), `UNAVAILABLE` (a `503`), `INTERNAL` (any
/// other `5xx`) —, RFC 6750 §3.1's error code for `INSUFFICIENT_SCOPE`, and
/// RFC 6585 §4's *rate limiting* for `RATE_LIMITED`.
pub mod code {
    use super::Code;

    /// No credential, or one that could not be verified — a `401`.
    pub const UNAUTHENTICATED: Code = Code::new("UNAUTHENTICATED");
    /// The caller is known and may not do this — a `403`.
    pub const FORBIDDEN: Code = Code::new("FORBIDDEN");
    /// The credential is valid but too narrow (RFC 6750 §3.1) — a `403`.
    pub const INSUFFICIENT_SCOPE: Code = Code::new("INSUFFICIENT_SCOPE");
    /// Too many requests; retry after the wait it names — a `429`.
    pub const RATE_LIMITED: Code = Code::new("RATE_LIMITED");
    /// Something the server depends on did not answer — a `503`.
    pub const UNAVAILABLE: Code = Code::new("UNAVAILABLE");
    /// The input is well formed and refused — a `400` or a `422`.
    pub const INVALID_ARGUMENT: Code = Code::new("INVALID_ARGUMENT");
    /// What was addressed does not exist, or is hidden from this caller — a `404`.
    pub const NOT_FOUND: Code = Code::new("NOT_FOUND");
    /// The request conflicts with the current state — a `409`.
    pub const CONFLICT: Code = Code::new("CONFLICT");
    /// The server failed; the reason is none of the client's business — a `500`.
    pub const INTERNAL: Code = Code::new("INTERNAL");
    /// What was addressed does not serve the request's method — a `405`.
    pub const METHOD_NOT_ALLOWED: Code = Code::new("METHOD_NOT_ALLOWED");
    /// No representation the request accepts can be answered — a `406`.
    pub const NOT_ACCEPTABLE: Code = Code::new("NOT_ACCEPTABLE");
    /// The request did not arrive whole in the time the server waits — a `408`.
    pub const REQUEST_TIMEOUT: Code = Code::new("REQUEST_TIMEOUT");
    /// What was addressed is gone for good — a `410`.
    pub const GONE: Code = Code::new("GONE");
    /// A precondition the request states does not hold — a `412`.
    pub const PRECONDITION_FAILED: Code = Code::new("PRECONDITION_FAILED");
    /// The request's content is larger than the server takes — a `413`.
    pub const CONTENT_TOO_LARGE: Code = Code::new("CONTENT_TOO_LARGE");
    /// The request's content is in a format the server does not take — a `415`.
    pub const UNSUPPORTED_MEDIA_TYPE: Code = Code::new("UNSUPPORTED_MEDIA_TYPE");
    /// No range the request names overlaps what is addressed — a `416`.
    pub const RANGE_NOT_SATISFIABLE: Code = Code::new("RANGE_NOT_SATISFIABLE");
    /// The request is served only once the connection switches protocols — a `426`.
    pub const UPGRADE_REQUIRED: Code = Code::new("UPGRADE_REQUIRED");
}

/// A client error every edge answers in its standard form.
///
/// Built in a `const` when nothing varies, where a status outside `400..=599`
/// is a compile error:
///
/// ```
/// use nest_rs_core::{Problem, code};
///
/// const TAKEN: Problem = Problem::new(409, code::CONFLICT);
///
/// let problem = TAKEN.with_detail("the handle is taken");
/// assert_eq!(problem.detail(), Some("the handle is taken"));
/// assert_eq!(problem.to_string(), "409 CONFLICT");
/// ```
///
/// ```compile_fail,E0080
/// use nest_rs_core::{Problem, code};
///
/// const FINE: Problem = Problem::new(200, code::CONFLICT);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    status: u16,
    code: Code,
    /// The type's URI and its title, which RFC 9457 pairs.
    problem_type: Option<(&'static str, &'static str)>,
    detail: Option<Cow<'static, str>>,
    retry_after: Option<u32>,
    // Boxed so a `Result<_, Problem>` stays the size of the members most
    // problems carry.
    errors: Option<Box<serde_json::Value>>,
}

impl Problem {
    /// A problem answering `status`, a `4xx` or a `5xx`, known by `code`.
    ///
    /// # Panics
    ///
    /// When `status` is outside `400..=599`. Declared in a `const`, the panic is
    /// a compile error.
    #[expect(
        clippy::panic,
        reason = "evaluated in a const, where a panic is the compile error naming the refused status"
    )]
    pub const fn new(status: u16, code: Code) -> Self {
        if status < 400 || status > 599 {
            panic!("a `Problem` answers a client error or a server error: a status in 400..=599");
        }
        Self {
            status,
            code,
            problem_type: None,
            detail: None,
            retry_after: None,
            errors: None,
        }
    }

    /// This problem's type (RFC 9457 §3.1.1): the URI naming an app's own
    /// class, which a client may key on and dereference for its
    /// documentation — an absolute URI, as the RFC recommends — and its
    /// title, a short summary the same for every occurrence (§3.1.3). Unset,
    /// the type is `about:blank`, a problem meaning nothing beyond its status
    /// and [`code`](Self::code), whose title is the status's reason phrase.
    ///
    /// # Panics
    ///
    /// When `uri` is empty or holds a character no URI reference does (RFC
    /// 3986 §2), or `title` is empty. Declared in a `const`, the panic is a
    /// compile error:
    ///
    /// ```compile_fail,E0080
    /// use nest_rs_core::{Problem, code};
    ///
    /// const TAKEN: Problem =
    ///     Problem::new(409, code::CONFLICT).with_type("handle taken", "Handle taken");
    /// ```
    #[expect(
        clippy::panic,
        reason = "evaluated in a const, where a panic is the compile error naming the refused type"
    )]
    pub const fn with_type(mut self, uri: &'static str, title: &'static str) -> Self {
        let bytes = uri.as_bytes();
        if bytes.is_empty() {
            panic!("a problem `type` is a URI reference, not empty");
        }
        let mut i = 0;
        while i < bytes.len() {
            if !is_uri_byte(bytes[i]) {
                panic!(
                    "a problem `type` is a URI reference (RFC 3986 §2): letters, digits, \
                     `-._~:/?#[]@!$&'()*+,;=` and percent-encoded bytes"
                );
            }
            i += 1;
        }
        if title.is_empty() {
            panic!("a problem `title` summarizes its type, not empty");
        }
        self.problem_type = Some((uri, title));
        self
    }

    /// A sentence the client may read. A server error never carries one to
    /// the wire, [`detail`](Self::detail) answering `None` on a `5xx`, nor to
    /// a log line: what failed belongs in the error's chain, which an edge
    /// files at `error` when its answer withholds it.
    pub fn with_detail(mut self, detail: impl Into<Cow<'static, str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// How long the client should wait before retrying, in seconds: HTTP's
    /// `Retry-After`, `retryAfterSeconds` on the edges with no header.
    pub const fn with_retry_after(mut self, seconds: u32) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    /// The field map a client acts on field by field, as a pipe's refusal
    /// carries it — every edge's `errors` member, and nothing else is. Never
    /// one of the values the request sent. A server error never carries it to
    /// the wire, [`errors`](Self::errors) answering `None` on a `5xx`.
    ///
    /// ```
    /// use nest_rs_core::{Problem, code};
    ///
    /// let fields = serde_json::json!({ "email": [{ "code": "email" }] });
    /// let problem = Problem::new(422, code::INVALID_ARGUMENT).with_errors(fields.clone());
    /// assert_eq!(problem.errors(), Some(&fields));
    /// ```
    pub fn with_errors(mut self, errors: serde_json::Value) -> Self {
        self.errors = Some(Box::new(errors));
        self
    }

    /// The status the problem answers, `400..=599`.
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// The machine-readable class a client branches on.
    pub const fn code(&self) -> Code {
        self.code
    }

    /// The URI naming the problem's type, when [`with_type`](Self::with_type)
    /// set one; `None` is `about:blank`.
    pub const fn type_uri(&self) -> Option<&'static str> {
        match self.problem_type {
            Some((uri, _)) => Some(uri),
            None => None,
        }
    }

    /// The title of the problem's type, when [`with_type`](Self::with_type)
    /// set one.
    pub const fn title(&self) -> Option<&'static str> {
        match self.problem_type {
            Some((_, title)) => Some(title),
            None => None,
        }
    }

    /// The field map [`with_errors`](Self::with_errors) set on a `4xx`, and
    /// `None` on a `5xx`, whatever was set.
    pub fn errors(&self) -> Option<&serde_json::Value> {
        if self.is_server_error() {
            return None;
        }
        self.errors.as_deref()
    }

    /// The sentence the client reads: what [`with_detail`](Self::with_detail)
    /// set on a `4xx`, and `None` on a `5xx`, whatever was set.
    pub fn detail(&self) -> Option<&str> {
        if self.is_server_error() {
            return None;
        }
        self.detail.as_deref()
    }

    /// The wait before a retry, in seconds, when one was given.
    pub const fn retry_after(&self) -> Option<u32> {
        self.retry_after
    }

    /// Whether the server failed (`5xx`) rather than the request (`4xx`).
    pub const fn is_server_error(&self) -> bool {
        self.status >= 500
    }

    /// The one sentence an edge with no separate detail member answers (a
    /// GraphQL `message`, a WebSocket frame's `error`, an MCP error's
    /// `message`): the [`detail`](Self::detail) of a `4xx`, else its code in
    /// lowercase words; on a `5xx`, [`UNAVAILABLE_CLIENT_MESSAGE`] for a `503`
    /// and [`OPAQUE_CLIENT_MESSAGE`] otherwise.
    ///
    /// ```
    /// use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, Problem, code};
    ///
    /// assert_eq!(Problem::new(404, code::NOT_FOUND).client_message(), "not found");
    /// let failed = Problem::new(500, code::INTERNAL).with_detail("disk full");
    /// assert_eq!(failed.client_message(), OPAQUE_CLIENT_MESSAGE);
    /// ```
    pub fn client_message(&self) -> Cow<'static, str> {
        match self.status {
            503 => Cow::Borrowed(UNAVAILABLE_CLIENT_MESSAGE),
            500..=599 => Cow::Borrowed(OPAQUE_CLIENT_MESSAGE),
            _ => self
                .detail
                .clone()
                .unwrap_or_else(|| Cow::Owned(self.code.in_words())),
        }
    }
}

/// The status and the code, never the detail: a `Problem` read in a log line
/// says which answer was given, not what the client was told.
impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.status, self.code)
    }
}

impl Error for Problem {}

/// Whether `byte` may appear in a URI reference (RFC 3986 §2): unreserved,
/// reserved, or the `%` of a percent-encoded octet.
const fn is_uri_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b':'
            | b'/'
            | b'?'
            | b'#'
            | b'['
            | b']'
            | b'@'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b'%'
    )
}

/// An error that says what its client may read, implemented once by a domain
/// error type: the one mapping of its variants to the wire, on every edge.
///
/// The handler wrappers of every edge probe a handler's error for it by type —
/// HTTP's `#[routes]`, GraphQL's `#[operations]`, WebSocket's `#[messages]`,
/// MCP's `#[tools]` — and where a body answers the edge's own error, `?`
/// converts one on HTTP, whose `ResponseError` every `ToProblem` error is, and
/// `problem_error` on GraphQL and MCP (`nest_rs_graphql::problem_error`,
/// `nest_rs_mcp::problem_error`), whose error types are their libraries'.
/// `Some(problem)` answers that problem in the edge's standard form; `None`
/// answers opaquely. Either way, what the answer withholds of the error's
/// chain is logged at `error`, at `warn` behind a `503`.
pub trait ToProblem: Error + Send + Sync + 'static {
    /// The problem to answer, or `None` to answer opaquely.
    fn to_problem(&self) -> Option<Problem>;
}

/// A `Problem` returned as an error answers itself.
impl ToProblem for Problem {
    fn to_problem(&self) -> Option<Problem> {
        Some(self.clone())
    }
}

/// An error that cannot happen says no problem, so a `?` on an infallible
/// result converts wherever a `ToProblem` error does.
impl ToProblem for std::convert::Infallible {
    fn to_problem(&self) -> Option<Problem> {
        match *self {}
    }
}

/// Re-exported through the crate's `__private`.
pub(crate) mod __private {
    use std::error::Error;
    use std::time::Duration;

    use super::Problem;

    /// The `Retry-After` a `wait` answers with, in whole seconds: rounded up,
    /// since a client that waits less may collide with the attempt, and never
    /// `0`, which RFC 9110 §10.2.3 reads as "retry immediately".
    pub fn retry_after_secs(wait: Duration) -> u32 {
        let whole = wait
            .as_secs()
            .saturating_add(u64::from(wait.subsec_nanos() > 0))
            .max(1);
        u32::try_from(whole).unwrap_or(u32::MAX)
    }

    /// What answering `problem` for `error` keeps from the client: `error`'s
    /// whole chain when the answer is a `5xx` and `error` is more than the bare
    /// problem, else `None`.
    pub fn withheld(problem: &Problem, error: &(dyn Error + 'static)) -> Option<Withheld> {
        if error.is::<Problem>() {
            return None;
        }
        Withheld::behind(problem.status(), error)
    }

    /// The chain a `5xx` answer keeps from its client, and the level the edge
    /// files it at.
    #[derive(Debug, PartialEq, Eq)]
    #[expect(
        clippy::exhaustive_enums,
        reason = "tier 2: each edge's filing matches it, so a new level reaches every edge"
    )]
    pub enum Withheld {
        /// The server failed: filed at `error`, as an opaque failure is.
        Failed(String),
        /// The server could not serve the unit in time (a `503`) — a statement
        /// cancelled at its bound, a backend past its net: filed at `warn`.
        Unavailable(String),
    }

    impl Withheld {
        /// What a `status` answer keeps from the client when `error` is behind
        /// it: `error`'s whole chain for a `5xx`, else `None`.
        pub fn behind(status: u16, error: &(dyn Error + 'static)) -> Option<Self> {
            (status >= 500).then(|| {
                let said = crate::error_message(error);
                if status == 503 {
                    Self::Unavailable(said)
                } else {
                    Self::Failed(said)
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_says_its_status_and_code_and_never_its_detail() {
        let problem = Problem::new(409, code::CONFLICT).with_detail("user ada@example.com exists");
        let said = problem.to_string();
        assert_eq!(said, "409 CONFLICT");
        assert!(!said.contains("ada@example.com"), "{said}");
        assert!(!format!("{problem:?}").is_empty());
    }

    #[test]
    fn a_server_error_never_hands_its_detail_to_an_edge() {
        let problem = Problem::new(500, code::INTERNAL).with_detail("db-primary refused");
        assert_eq!(problem.detail(), None);
        assert_eq!(problem.client_message(), OPAQUE_CLIENT_MESSAGE);

        let unavailable = Problem::new(503, code::UNAVAILABLE)
            .with_detail("store at 10.0.0.1 down")
            .with_retry_after(7);
        assert_eq!(unavailable.detail(), None);
        assert_eq!(unavailable.client_message(), UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(unavailable.retry_after(), Some(7));
    }

    #[test]
    fn a_client_error_says_its_detail_or_its_code_in_words() {
        let detailed =
            Problem::new(422, code::INVALID_ARGUMENT).with_detail("amount must be positive");
        assert_eq!(detailed.detail(), Some("amount must be positive"));
        assert_eq!(detailed.client_message(), "amount must be positive");

        assert_eq!(
            Problem::new(404, code::NOT_FOUND).client_message(),
            "not found"
        );
        assert_eq!(
            Problem::new(403, code::INSUFFICIENT_SCOPE).client_message(),
            "insufficient scope"
        );
        assert_eq!(
            Problem::new(409, code::CONFLICT).client_message(),
            "conflict"
        );
    }

    #[test]
    fn a_problem_carries_its_type_title_and_field_map() {
        const TAKEN: Problem = Problem::new(409, code::CONFLICT)
            .with_type("https://example.com/problems/handle-taken", "Handle taken");
        let fields = serde_json::json!({ "handle": [{ "code": "taken" }] });
        let problem = TAKEN
            .with_detail("@ada is taken")
            .with_errors(fields.clone());
        assert_eq!(
            problem.type_uri(),
            Some("https://example.com/problems/handle-taken")
        );
        assert_eq!(problem.title(), Some("Handle taken"));
        assert_eq!(problem.errors(), Some(&fields));
        assert_eq!(
            problem.to_string(),
            "409 CONFLICT",
            "a log line says the class"
        );

        let blank = Problem::new(404, code::NOT_FOUND);
        assert_eq!(
            (blank.type_uri(), blank.title(), blank.errors()),
            (None, None, None)
        );
    }

    #[test]
    fn a_server_error_never_hands_its_field_map_to_an_edge() {
        let problem = Problem::new(500, code::INTERNAL)
            .with_type("https://example.com/problems/ledger-down", "Ledger down")
            .with_errors(serde_json::json!({ "amount": ["10.0.0.1 refused"] }));
        assert_eq!(problem.errors(), None);
        assert_eq!(
            problem.title(),
            Some("Ledger down"),
            "a type is no occurrence"
        );
    }

    #[test]
    fn a_type_is_a_uri_reference_with_a_title() {
        for accepted in [
            "https://example.com/problems/out-of-credit",
            "urn:problem:out-of-credit",
            "/problems/out-of-credit#v2",
            "tag:example.com,2026:caf%C3%A9",
        ] {
            assert_eq!(
                Problem::new(400, code::INVALID_ARGUMENT)
                    .with_type(accepted, "Out of credit")
                    .type_uri(),
                Some(accepted),
            );
        }
        for refused in ["", "out of credit", "https://example.com/é", "a\"b", "a<b>"] {
            let outcome = std::panic::catch_unwind(|| {
                Problem::new(400, code::INVALID_ARGUMENT).with_type(refused, "Out of credit")
            });
            assert!(outcome.is_err(), "{refused:?} is not a URI reference");
        }
        let untitled = std::panic::catch_unwind(|| {
            Problem::new(400, code::INVALID_ARGUMENT).with_type("urn:problem:out-of-credit", "")
        });
        assert!(untitled.is_err(), "a type has a title");
    }

    #[test]
    fn a_problem_returned_as_an_error_answers_itself() {
        let problem = Problem::new(429, code::RATE_LIMITED).with_retry_after(30);
        assert_eq!(problem.to_problem(), Some(problem.clone()));
    }

    #[test]
    fn a_wait_is_said_in_whole_seconds_rounded_up_and_never_zero() {
        use std::time::Duration;

        use super::__private::retry_after_secs;

        assert_eq!(retry_after_secs(Duration::from_secs(30)), 30);
        assert_eq!(retry_after_secs(Duration::new(30, 1)), 31);
        assert_eq!(retry_after_secs(Duration::from_millis(2500)), 3);
        assert_eq!(retry_after_secs(Duration::from_millis(500)), 1);
        assert_eq!(retry_after_secs(Duration::ZERO), 1);
        assert_eq!(retry_after_secs(Duration::MAX), u32::MAX);
    }

    #[test]
    fn a_server_answer_withholds_the_chain_behind_the_problem_and_nothing_else() {
        use super::__private::withheld;

        use super::__private::Withheld;

        let unavailable = Problem::new(503, code::UNAVAILABLE);
        let outage = std::io::Error::other("the index at 10.0.0.1 refused");
        assert_eq!(
            withheld(&unavailable, &outage),
            Some(Withheld::Unavailable(
                "the index at 10.0.0.1 refused".to_owned()
            )),
            "an answer the server could not give in time is filed at warn",
        );
        assert_eq!(
            withheld(&Problem::new(500, code::INTERNAL), &outage),
            Some(Withheld::Failed("the index at 10.0.0.1 refused".to_owned())),
        );
        assert_eq!(withheld(&unavailable, &unavailable), None, "a bare problem");
        let conflict = Problem::new(409, code::CONFLICT);
        assert_eq!(withheld(&conflict, &outage), None, "a client answer");
    }

    #[test]
    fn every_framework_code_reads_as_declared() {
        let all = [
            code::UNAUTHENTICATED,
            code::FORBIDDEN,
            code::INSUFFICIENT_SCOPE,
            code::RATE_LIMITED,
            code::UNAVAILABLE,
            code::INVALID_ARGUMENT,
            code::NOT_FOUND,
            code::CONFLICT,
            code::INTERNAL,
            code::METHOD_NOT_ALLOWED,
            code::NOT_ACCEPTABLE,
            code::REQUEST_TIMEOUT,
            code::GONE,
            code::PRECONDITION_FAILED,
            code::CONTENT_TOO_LARGE,
            code::UNSUPPORTED_MEDIA_TYPE,
            code::RANGE_NOT_SATISFIABLE,
            code::UPGRADE_REQUIRED,
        ];
        let distinct: std::collections::BTreeSet<&str> = all.iter().map(|c| c.as_str()).collect();
        assert_eq!(distinct.len(), all.len(), "a code is declared twice");
        assert_eq!(code::NOT_FOUND.to_string(), "NOT_FOUND");
        for (a, b) in all.iter().flat_map(|a| all.iter().map(move |b| (a, b))) {
            assert!(
                a == b || !b.as_str().starts_with(a.as_str()),
                "{a} is a raw prefix of {b}: a filter on one matches the other",
            );
        }
    }

    #[test]
    fn a_code_of_letters_digits_and_single_underscores_is_accepted() {
        assert_eq!(Code::new("V2_REQUIRED").as_str(), "V2_REQUIRED");
        assert_eq!(Code::new("A").as_str(), "A");
    }

    #[test]
    fn a_malformed_code_is_refused() {
        for refused in [
            "",
            "conflict",
            "_LEADING",
            "TRAILING_",
            "DOUBLE__UNDERSCORE",
            "9LIVES",
            "A_9",
            "SPACE D",
        ] {
            let outcome = std::panic::catch_unwind(|| Code::new(refused));
            assert!(outcome.is_err(), "{refused:?} is not a code");
        }
    }

    #[test]
    fn a_status_outside_the_error_classes_is_refused() {
        for refused in [0, 200, 302, 399, 600] {
            let outcome = std::panic::catch_unwind(|| Problem::new(refused, code::INTERNAL));
            assert!(outcome.is_err(), "{refused} is not an error status");
        }
        assert_eq!(Problem::new(400, code::INVALID_ARGUMENT).status(), 400);
        assert_eq!(Problem::new(599, code::INTERNAL).status(), 599);
    }
}

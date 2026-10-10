//! What a client is told when a unit fails: a [`Problem`], the [`Code`] it is
//! known by, and [`ToProblem`], the one conversion a domain error implements to
//! say it on every edge.
//!
//! Each edge answers a problem in its own standard form: an RFC 9457 problem
//! document with a `code` member on HTTP (`ProblemDetails` is its document),
//! `extensions.code` on GraphQL, `data.errors.code` in a WebSocket error frame,
//! `data.code` in an MCP JSON-RPC error. Where a [`ToProblem`] error is read
//! differs: GraphQL's and WebSocket's handler wrappers probe a handler's error
//! for it by type, an MCP operation converts one with
//! `nest_rs_mcp::problem_error`, and HTTP reads a [`Problem`] only — the error
//! itself or one its chain carries — beside an error's own `ResponseError`. An
//! error the edge finds no answer in is answered opaquely
//! ([`OPAQUE_CLIENT_MESSAGE`]) and its chain is logged once at `error`.
//!
//! ```
//! use nest_rs_core::problem::code;
//! use nest_rs_core::{Code, Problem, ToProblem};
//!
//! const IDEMPOTENCY_KEY_REQUIRED: Code = Code::new("IDEMPOTENCY_KEY_REQUIRED");
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
//!             Self::AlreadyPaid => Some(Problem::new(409, code::CONFLICT).with_detail("already paid")),
//!             Self::NoKey => Some(Problem::new(400, IDEMPOTENCY_KEY_REQUIRED)),
//!             Self::Ledger(_) => None,
//!         }
//!     }
//! }
//!
//! let problem = OrderError::AlreadyPaid.to_problem().expect("a client error");
//! assert_eq!((problem.status(), problem.code()), (409, code::CONFLICT));
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

/// The codes the framework answers with, one per outcome class: the HTTP
/// status's reason phrase when the class is an HTTP one, gRPC's canonical
/// status name otherwise.
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
}

/// A client error every edge answers in its standard form.
///
/// Built in a `const` when nothing varies, where a status outside `400..=599`
/// is a compile error:
///
/// ```
/// use nest_rs_core::Problem;
/// use nest_rs_core::problem::code;
///
/// const TAKEN: Problem = Problem::new(409, code::CONFLICT);
///
/// let problem = TAKEN.with_detail("the handle is taken");
/// assert_eq!(problem.detail(), Some("the handle is taken"));
/// assert_eq!(problem.to_string(), "409 CONFLICT");
/// ```
///
/// ```compile_fail,E0080
/// use nest_rs_core::Problem;
/// use nest_rs_core::problem::code;
///
/// const FINE: Problem = Problem::new(200, code::CONFLICT);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    status: u16,
    code: Code,
    detail: Option<Cow<'static, str>>,
    retry_after: Option<u32>,
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
            detail: None,
            retry_after: None,
        }
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

    /// The status the problem answers, `400..=599`.
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// The machine-readable class a client branches on.
    pub const fn code(&self) -> Code {
        self.code
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
    /// use nest_rs_core::problem::code;
    /// use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, Problem};
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

/// An error that says what its client may read, implemented once by a domain
/// error type.
///
/// GraphQL's `#[operations]` and WebSocket's `#[messages]` wrappers probe a
/// handler's error for it by type; an MCP operation converts one with
/// `nest_rs_mcp::problem_error`. HTTP does not read it: a route answers a
/// [`Problem`] its error is or carries, or the error's own `ResponseError`, so
/// a domain error a route returns implements that too. `Some(problem)` answers
/// that problem in the edge's standard form; `None` answers opaquely. Either
/// way, what the answer withholds of the error's chain is logged at `error`.
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

/// Re-exported through the crate's `__private`.
pub(crate) mod __private {
    use std::error::Error;

    use super::Problem;

    /// What answering `problem` for `error` keeps from the client, for the
    /// edge to file at `error` as it files an opaque failure: `error`'s whole
    /// chain when the answer is a `5xx` and `error` is more than the bare
    /// problem, else `None`.
    pub fn withheld(problem: &Problem, error: &(dyn Error + 'static)) -> Option<String> {
        (problem.is_server_error() && !error.is::<Problem>()).then(|| crate::error_message(error))
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
    fn a_problem_returned_as_an_error_answers_itself() {
        let problem = Problem::new(429, code::RATE_LIMITED).with_retry_after(30);
        assert_eq!(problem.to_problem(), Some(problem.clone()));
    }

    #[test]
    fn a_server_answer_withholds_the_chain_behind_the_problem_and_nothing_else() {
        use super::__private::withheld;

        let unavailable = Problem::new(503, code::UNAVAILABLE);
        let outage = std::io::Error::other("the index at 10.0.0.1 refused");
        assert_eq!(
            withheld(&unavailable, &outage).as_deref(),
            Some("the index at 10.0.0.1 refused"),
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
        ];
        let distinct: std::collections::BTreeSet<&str> = all.iter().map(|c| c.as_str()).collect();
        assert_eq!(distinct.len(), all.len(), "a code is declared twice");
        assert_eq!(code::NOT_FOUND.to_string(), "NOT_FOUND");
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

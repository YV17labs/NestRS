//! GraphQL error rendering shared by every rejection site, and the one rule a
//! resolver's `Err` is answered by.
//!
//! A resolver's error is rendered **by its type**, where the `#[operations]`
//! wrapper names it, each tier taken only when the one above does not apply:
//!
//! 1. an `async_graphql::Error` — the edge's own deliberate error — as built;
//! 2. a [`ToProblem`] error: the problem it says ([`problem_error`]), or opaque
//!    when it says none;
//! 3. framework vocabulary found in its chain: a [`Problem`], a [`PipeError`]
//!    ([`pipe_error`]), a decode failure said without its value;
//! 4. anything else: [`OPAQUE_CLIENT_MESSAGE`] under the `INTERNAL` code, and
//!    one `error` on `nest_rs::graphql` carrying the whole chain.
//!
//! An error that is none of these — a `Display` type that is no
//! `std::error::Error` — does not compile.
//!
//! [`OPAQUE_CLIENT_MESSAGE`]: nest_rs_core::OPAQUE_CLIENT_MESSAGE

use std::error::Error as StdError;

use async_graphql::{Error, ErrorExtensions};
use nest_rs_core::problem::code;
use nest_rs_core::{DecodeError, Problem, ToProblem};
use nest_rs_pipes::PipeError;

/// The extension member carrying field-level validation errors — the name
/// every other transport uses for them.
pub const FIELD_ERRORS_EXTENSION: &str = "errors";

/// The extension member carrying the [`Code`](nest_rs_core::Code) a client
/// branches on.
pub const CODE_EXTENSION: &str = "code";

/// Render a [`PipeError`] as an `async_graphql::Error`, carrying any structured
/// field errors under `extensions.errors`.
pub fn pipe_error(err: &PipeError) -> Error {
    let message = err.message().to_owned();
    match err.details() {
        // Plain JSON always converts; a failure would drop the extension, not the error.
        Some(details) => match async_graphql::Value::from_json(details.clone()) {
            Ok(value) => {
                Error::new(message).extend_with(move |_, e| e.set(FIELD_ERRORS_EXTENSION, value))
            }
            Err(_) => Error::new(message),
        },
        // Absent, not null: a client branches on presence.
        None => Error::new(message),
    }
}

/// Render what a [`ToProblem`] error says — a [`Problem`] among them — as an
/// `async_graphql::Error`: the problem's
/// [`client_message`](Problem::client_message), its code under
/// `extensions.code` and its wait under `extensions.retryAfterSeconds`; or,
/// when the error says none, the opaque answer, its chain filed at `error`.
///
/// A resolver returning the error answers through it already; a body mixing
/// failures inside an `async_graphql::Result` converts with it.
///
/// ```
/// use nest_rs_core::Problem;
/// use nest_rs_core::problem::code;
///
/// let error = nest_rs_graphql::problem_error(&Problem::new(409, code::CONFLICT));
/// assert_eq!(error.message, "conflict");
/// ```
pub fn problem_error<E: ToProblem>(error: &E) -> Error {
    match error.to_problem() {
        Some(problem) => rendered(&problem),
        None => opaque_error(error),
    }
}

/// A problem in GraphQL's form.
fn rendered(problem: &Problem) -> Error {
    let code = problem.code().as_str();
    let retry_after = problem.retry_after();
    Error::new(problem.client_message()).extend_with(move |_, e| {
        e.set(CODE_EXTENSION, code);
        if let Some(seconds) = retry_after {
            e.set("retryAfterSeconds", seconds);
        }
    })
}

/// The opaque answer, with `error`'s whole chain on the operator's line: what
/// a failure the client is owed no explanation for becomes.
pub(crate) fn opaque_error(error: &(dyn StdError + 'static)) -> Error {
    tracing::error!(
        target: crate::TARGET,
        error = %nest_rs_core::error_message(error),
        "graphql operation failed",
    );
    // The `INTERNAL` code an internal denial carries, so the two are indistinguishable.
    rendered(&Problem::new(500, code::INTERNAL))
}

/// Tiers 3 and 4: the framework vocabulary `error`'s chain carries, else the
/// opaque answer.
fn chain_error(error: &(dyn StdError + 'static)) -> Error {
    use nest_rs_core::__private::find_in_chain;

    if let Some(problem) = find_in_chain::<Problem>(error) {
        return rendered(problem);
    }
    if let Some(rejection) = find_in_chain::<PipeError>(error) {
        return pipe_error(rejection);
    }
    if let Some(report) = DecodeError::in_chain(error) {
        return rendered(
            &Problem::new(400, code::INVALID_ARGUMENT).with_detail(report.to_string()),
        );
    }
    opaque_error(error)
}

/// A resolver's error, rendered by its type (the module's tiers): the inherent
/// method takes a [`ToProblem`] error, [`ErrorReportDeliberate`] an
/// `async_graphql::Error`, [`ErrorReportChain`] any other error.
pub struct ErrorReport<E>(pub E);

impl<E: ToProblem> ErrorReport<E> {
    /// Tier 2: the problem the error says, or the opaque answer.
    pub fn into_graphql_error(self) -> Error {
        problem_error(&self.0)
    }
}

/// Tier 1 of [`ErrorReport`]: the edge's own error, as built.
pub trait ErrorReportDeliberate {
    /// The error itself.
    fn into_graphql_error(self) -> Error;
}

impl ErrorReportDeliberate for ErrorReport<Error> {
    fn into_graphql_error(self) -> Error {
        self.0
    }
}

/// Tiers 3 and 4 of [`ErrorReport`]: any error the inherent method and
/// [`ErrorReportDeliberate`] do not take.
pub trait ErrorReportChain {
    /// The vocabulary its chain carries, else the opaque answer.
    fn into_graphql_error(self) -> Error;
}

impl<E> ErrorReportChain for ErrorReport<E>
where
    E: Into<Box<dyn StdError + Send + Sync>> + 'static,
{
    fn into_graphql_error(self) -> Error {
        chain_error(&*nest_rs_core::boxed_error(self.0))
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::OPAQUE_CLIENT_MESSAGE;

    use super::*;

    fn extensions(err: &Error) -> serde_json::Value {
        serde_json::to_value(&err.extensions).expect("extensions serialize")
    }

    #[test]
    fn a_rejection_with_details_carries_them_under_the_errors_extension() {
        let details = serde_json::json!({
            "name": [{ "code": "length", "params": { "min": 1 } }],
            "email": [{ "code": "email" }],
        });
        let err = pipe_error(&PipeError::with_details(
            "validation failed",
            details.clone(),
        ));

        assert_eq!(err.message, "validation failed");
        assert_eq!(extensions(&err)[FIELD_ERRORS_EXTENSION], details);
    }

    #[test]
    fn a_rejection_without_details_carries_no_extensions() {
        let err = pipe_error(&PipeError::new("must be a valid UUID"));
        assert_eq!(err.message, "must be a valid UUID");
        assert!(
            err.extensions.is_none(),
            "absent, not null — a client branches on presence",
        );
    }

    #[test]
    fn a_problem_answers_its_sentence_its_code_and_its_wait() {
        let conflict =
            problem_error(&Problem::new(409, code::CONFLICT).with_detail("the handle is taken"));
        assert_eq!(conflict.message, "the handle is taken");
        assert_eq!(extensions(&conflict)[CODE_EXTENSION], "CONFLICT");
        assert!(extensions(&conflict).get("retryAfterSeconds").is_none());

        let limited = problem_error(&Problem::new(429, code::RATE_LIMITED).with_retry_after(30));
        assert_eq!(limited.message, "rate limited");
        assert_eq!(extensions(&limited)["retryAfterSeconds"], 30);
    }

    #[test]
    fn a_server_problem_never_answers_its_detail() {
        let failed =
            problem_error(&Problem::new(500, code::INTERNAL).with_detail("db-primary refused"));
        assert_eq!(failed.message, OPAQUE_CLIENT_MESSAGE);
        assert_eq!(extensions(&failed)[CODE_EXTENSION], "INTERNAL");
    }

    #[derive(Debug)]
    struct Ledger;

    impl std::fmt::Display for Ledger {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the ledger at 10.0.0.1 refused")
        }
    }

    impl StdError for Ledger {}

    #[derive(Debug)]
    enum Payment {
        Declined,
        Ledger(Ledger),
    }

    impl std::fmt::Display for Payment {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(match self {
                Self::Declined => "the card was declined",
                Self::Ledger(_) => "the ledger failed",
            })
        }
    }

    impl StdError for Payment {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                Self::Declined => None,
                Self::Ledger(ledger) => Some(ledger),
            }
        }
    }

    impl ToProblem for Payment {
        fn to_problem(&self) -> Option<Problem> {
            match self {
                Self::Declined => Some(Problem::new(
                    402,
                    nest_rs_core::Code::new("PAYMENT_DECLINED"),
                )),
                Self::Ledger(_) => None,
            }
        }
    }

    #[test]
    fn each_error_is_rendered_by_its_type() {
        use super::ErrorReportDeliberate as _;

        let deliberate = ErrorReport(Error::new("bad input")).into_graphql_error();
        assert_eq!(deliberate.message, "bad input");
        assert!(deliberate.extensions.is_none());

        let declined = ErrorReport(Payment::Declined).into_graphql_error();
        assert_eq!(declined.message, "payment declined");
        assert_eq!(extensions(&declined)[CODE_EXTENSION], "PAYMENT_DECLINED");

        let logs = nest_rs_testing::LogCapture::install();
        let ledger = ErrorReport(Payment::Ledger(Ledger)).into_graphql_error();
        assert_eq!(ledger.message, OPAQUE_CLIENT_MESSAGE);
        let event = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert_eq!(event.level, "error");
        assert!(
            event.field("error").is_some_and(|e| e.contains("10.0.0.1")),
            "{event:#?}"
        );
    }

    #[test]
    fn the_vocabulary_a_chain_carries_is_answered_and_anything_else_is_opaque() {
        use super::ErrorReportChain as _;

        let wrapped = nest_rs_core::anyhow::Error::from(Problem::new(404, code::NOT_FOUND))
            .context("loading the post");
        let found = ErrorReport(wrapped).into_graphql_error();
        assert_eq!(found.message, "not found");
        assert_eq!(extensions(&found)[CODE_EXTENSION], "NOT_FOUND");

        let piped = ErrorReport(PipeError::new("must be a valid UUID")).into_graphql_error();
        assert_eq!(piped.message, "must be a valid UUID");

        let decode = serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        let said = ErrorReport(decode).into_graphql_error();
        assert!(!said.message.contains("sk_live"), "{}", said.message);
        assert_eq!(extensions(&said)[CODE_EXTENSION], "INVALID_ARGUMENT");

        let logs = nest_rs_testing::LogCapture::install();
        let secret = ErrorReport(std::io::Error::other("secret 42")).into_graphql_error();
        assert_eq!(secret.message, OPAQUE_CLIENT_MESSAGE);
        assert_eq!(extensions(&secret)[CODE_EXTENSION], "INTERNAL");
        let event = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("secret 42"))
        );
    }
}

//! GraphQL error rendering shared by every rejection site, and the one rule a
//! field's failure is answered by.
//!
//! A resolver's error is rendered **by its type**, where the `#[operations]`
//! wrapper names it ([`ErrorReport`]), each tier taken only when the one above
//! does not apply:
//!
//! 1. an `async_graphql::Error` — the edge's own deliberate error — as built;
//! 2. a [`ToProblem`] error: the problem it says ([`problem_error`]), or opaque
//!    when it says none;
//! 3. framework vocabulary found in its chain: a [`Problem`], a [`PipeError`]
//!    ([`pipe_error`]), a decode failure said without its value;
//! 4. anything else: [`OPAQUE_CLIENT_MESSAGE`] under the `INTERNAL` code, and
//!    one `error` on `nest_rs::graphql` carrying the whole chain.
//!
//! A `5xx` problem answers the sentence an opaque failure answers, so the chain
//! behind it is filed the same way; a bare `Problem` withholds nothing. What a
//! field answers is final only once its unit ran — an exception filter may
//! answer it otherwise — so a chain is filed there, by the answer the client
//! gets.
//!
//! An error that is none of these — a `Display` type that is no
//! `std::error::Error` — does not compile.
//!
//! [`OPAQUE_CLIENT_MESSAGE`]: nest_rs_core::OPAQUE_CLIENT_MESSAGE

use std::error::Error as StdError;

use async_graphql::{Error, ErrorExtensions};
use nest_rs_core::__private::{Withheld, caught_problem};
use nest_rs_core::problem::code;
use nest_rs_core::{DecodeError, Failure, Problem, ToProblem};
use nest_rs_guards::Denial;
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

/// Render a guard's [`Denial`] as an `async_graphql::Error`: its message, its
/// `extensions.code`, and the scopes or the wait it names.
///
/// The required scopes of a scope denial ride as a `requiredScopes` list
/// extension, since an error frame has no `401` to enrich. A `5xx` answers its
/// condition's sentence, never the guard's text.
pub fn denial_to_graphql_error(denial: Denial) -> Error {
    use nest_rs_guards::__private::{client_message, denial_code};

    let code = denial_code(&denial).as_str();
    let scopes = denial.required_scopes();
    let required = (!scopes.is_empty()).then(|| scopes.to_vec());
    let retry_after = denial.retry_after_secs();
    Error::new(client_message(&denial)).extend_with(move |_, e| {
        e.set(CODE_EXTENSION, code);
        if let Some(required) = required {
            e.set("requiredScopes", required);
        }
        if let Some(retry_after) = retry_after {
            e.set("retryAfterSeconds", retry_after);
        }
    })
}

/// Render what a [`ToProblem`] error says — a [`Problem`] among them — as an
/// `async_graphql::Error`: the problem's
/// [`client_message`](Problem::client_message), its code under
/// `extensions.code` and its wait under `extensions.retryAfterSeconds`; or,
/// when the error says none, the opaque answer. Either way the chain an answer
/// withholds — the opaque one's, a `5xx` problem's — is filed at `error`.
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
    problem_reply(error).filed()
}

/// `problem` as the answer to `error`, filing what a `5xx` answer withholds.
fn answered(problem: &Problem, error: &(dyn StdError + 'static)) -> Error {
    Reply::problem(problem, error).filed()
}

/// A problem in GraphQL's form.
pub(crate) fn rendered(problem: &Problem) -> Error {
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
    Reply::opaque(error).filed()
}

/// The operator's line for a failure the answer withholds: its whole chain.
fn failed(said: &str) {
    tracing::error!(
        target: crate::TARGET,
        error = %said,
        "graphql operation failed",
    );
}

/// The operator's line for a `503` the answer withholds the cause of: the
/// server could not serve it in time, which is no failure of its own.
fn unavailable(said: &str) {
    tracing::warn!(
        target: crate::TARGET,
        error = %said,
        "graphql operation unavailable",
    );
}

/// Tiers 3 and 4: the framework vocabulary `error`'s chain carries, else the
/// opaque answer.
fn chain_error(error: &(dyn StdError + 'static)) -> Error {
    use nest_rs_core::__private::find_in_chain;

    if let Some(problem) = find_in_chain::<Problem>(error) {
        return answered(problem, error);
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

/// Why a field's tail failed: the failure the pipeline carries, which its
/// exception filters read, and the answer only the error's type could say,
/// held until the field's unit decides whether it goes out.
pub struct FieldFailure {
    pub(crate) failure: Failure,
    pub(crate) reply: Option<Box<Reply>>,
}

/// An answer a field wrote, with the chain it keeps from the client — filed
/// only if it is the one sent.
pub(crate) struct Reply {
    error: Error,
    withheld: Option<Withheld>,
}

impl Reply {
    /// `problem` as the answer to `error`, keeping what a `5xx` answer
    /// withholds.
    fn problem(problem: &Problem, error: &(dyn StdError + 'static)) -> Self {
        Self {
            error: rendered(problem),
            withheld: nest_rs_core::__private::withheld(problem, error),
        }
    }

    /// The opaque answer, keeping `error`'s whole chain.
    fn opaque(error: &(dyn StdError + 'static)) -> Self {
        Self {
            // The `INTERNAL` code an internal denial carries, so the two are
            // indistinguishable.
            error: rendered(&Problem::new(500, code::INTERNAL)),
            withheld: Some(Withheld::Failed(nest_rs_core::error_message(error))),
        }
    }

    /// The answer, its withheld chain filed: it is the one sent.
    pub(crate) fn filed(self) -> Error {
        match self.withheld {
            Some(Withheld::Failed(said)) => failed(&said),
            Some(Withheld::Unavailable(said)) => unavailable(&said),
            None => {}
        }
        self.error
    }
}

impl FieldFailure {
    /// A failure the edge's one rule answers once the unit ran.
    pub(crate) fn unanswered(failure: Failure) -> Self {
        Self {
            failure,
            reply: None,
        }
    }

    /// What a field failing with this answers when no exception filter
    /// answers it, the chain it withholds filed.
    pub fn into_answer(self) -> Error {
        match self.reply {
            Some(reply) => reply.filed(),
            None => unanswered(&self.failure),
        }
    }
}

/// The edge's own error, as built: its own answer.
impl From<Error> for FieldFailure {
    fn from(error: Error) -> Self {
        Self {
            failure: Failure::from(DeliberateError),
            reply: Some(Box::new(Reply {
                error,
                withheld: None,
            })),
        }
    }
}

/// A per-argument pipe's refusal, answered as one.
impl From<PipeError> for FieldFailure {
    fn from(refused: PipeError) -> Self {
        Self::unanswered(Failure::from(refused))
    }
}

/// What the pipeline carries for a field that answered its own
/// `async_graphql::Error`: the answer is that error, as built, so the failure
/// names no more than this.
#[derive(Debug, thiserror::Error)]
#[error("the field answered its own GraphQL error")]
pub(crate) struct DeliberateError;

/// A provider a field reaches that neither its request nor the container
/// holds: the type goes on the operator's line, never to the client.
#[derive(Debug, thiserror::Error)]
#[error(
    "no `{provider}` reached the field — list it in a module's providers, and serve a \
     request-scoped one over the HTTP request"
)]
pub(crate) struct Unprovided {
    pub(crate) provider: &'static str,
}

/// A federation field reached on a schema that carries no container, so no
/// site to run it on: refused rather than served past its guards.
#[derive(Debug, thiserror::Error)]
#[error(
    "the schema carries no container, so `{field}` has no site to run on — serve it through \
     GraphqlModule, which attaches one"
)]
pub(crate) struct Unserved {
    pub(crate) field: &'static str,
}

/// What a field whose unit failed answers: an exception filter's problem over
/// whatever it wrote, else the answer it wrote, else the edge's one rule —
/// each filing the chain it keeps from the client.
pub(crate) fn answer(failure: &Failure, reply: Option<Box<Reply>>) -> Error {
    if let Some(problem) = caught_problem(failure) {
        return answered(problem, failure.error());
    }
    match reply {
        Some(reply) if failure.is_answered() => reply.filed(),
        _ => unanswered(failure),
    }
}

/// The edge's one rule for a failure nothing answered: a refusal as its
/// denial, the vocabulary its chain carries, else the opaque answer.
fn unanswered(failure: &Failure) -> Error {
    match failure.find::<Denial>() {
        Some(denial) => denial_to_graphql_error(denial.clone()),
        None => chain_error(failure.error()),
    }
}

/// The answer to `error`'s [`ToProblem`] — the problem it says, or the opaque
/// answer when it says none — and the chain it keeps from the client.
fn problem_reply<E: ToProblem>(error: &E) -> Reply {
    match error.to_problem() {
        Some(problem) => Reply::problem(&problem, error),
        None => Reply::opaque(error),
    }
}

/// A resolver's error, rendered by its type (the module's tiers): the inherent
/// method takes a [`ToProblem`] error, [`ErrorReportDeliberate`] an
/// `async_graphql::Error`, [`ErrorReportChain`] any other error.
pub struct ErrorReport<E>(pub E);

impl<E: ToProblem> ErrorReport<E> {
    /// Tier 2: the problem the error says, or the opaque answer.
    pub fn into_failure(self) -> FieldFailure {
        let reply = Box::new(problem_reply(&self.0));
        FieldFailure {
            failure: Failure::from(self.0),
            reply: Some(reply),
        }
    }
}

/// Tier 1 of [`ErrorReport`]: the edge's own error, as built.
pub trait ErrorReportDeliberate {
    /// The error itself.
    fn into_failure(self) -> FieldFailure;
}

impl ErrorReportDeliberate for ErrorReport<Error> {
    fn into_failure(self) -> FieldFailure {
        FieldFailure::from(self.0)
    }
}

/// Tiers 3 and 4 of [`ErrorReport`]: any error the inherent method and
/// [`ErrorReportDeliberate`] do not take.
pub trait ErrorReportChain {
    /// The vocabulary its chain carries, else the opaque answer.
    fn into_failure(self) -> FieldFailure;
}

impl<E> ErrorReportChain for ErrorReport<E>
where
    E: Into<Box<dyn StdError + Send + Sync>> + 'static,
{
    fn into_failure(self) -> FieldFailure {
        FieldFailure::unanswered(Failure::new(self.0))
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::OPAQUE_CLIENT_MESSAGE;

    use super::*;

    fn extensions(err: &Error) -> serde_json::Value {
        serde_json::to_value(&err.extensions).expect("extensions serialize")
    }

    /// What an unavailable dependency's refusal said of it stays on the server;
    /// the class a client branches on and the wait it acts on travel.
    #[test]
    fn an_unavailable_denial_tells_the_client_its_condition_and_its_wait_only() {
        let err = denial_to_graphql_error(Denial::unavailable(Some(7), "store at 10.0.0.1 down"));
        assert_eq!(err.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        let extensions = extensions(&err);
        assert!(!extensions.to_string().contains("10.0.0.1"), "{extensions}");
        assert_eq!(extensions["code"], "UNAVAILABLE");
        assert_eq!(extensions["retryAfterSeconds"], 7);
    }

    #[test]
    fn an_internal_denial_answers_the_opaque_message() {
        let err = denial_to_graphql_error(Denial::internal("redis at 10.0.3.7 refused"));
        assert_eq!(err.message, OPAQUE_CLIENT_MESSAGE);
        assert_eq!(extensions(&err)["code"], "INTERNAL");
    }

    /// A client branches on one member, `code`, whether a guard refused it or a
    /// `Problem` answered it.
    #[test]
    fn a_denial_names_its_class_by_the_code_a_problem_carries() {
        for (denial, code) in [
            (Denial::unauthorized("no token"), "UNAUTHENTICATED"),
            (Denial::forbidden("not yours"), "FORBIDDEN"),
            (
                Denial::insufficient_scope(["posts:write"], "too narrow"),
                "INSUFFICIENT_SCOPE",
            ),
            (Denial::rate_limited(7, "slow down"), "RATE_LIMITED"),
        ] {
            let extensions = extensions(&denial_to_graphql_error(denial));
            assert_eq!(extensions["code"], code, "{extensions}");
        }
        let scoped = extensions(&denial_to_graphql_error(Denial::insufficient_scope(
            ["posts:write"],
            "too narrow",
        )));
        assert_eq!(scoped["requiredScopes"], serde_json::json!(["posts:write"]));
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

        let deliberate = ErrorReport(Error::new("bad input"))
            .into_failure()
            .into_answer();
        assert_eq!(deliberate.message, "bad input");
        assert!(deliberate.extensions.is_none());

        let declined = ErrorReport(Payment::Declined).into_failure().into_answer();
        assert_eq!(declined.message, "payment declined");
        assert_eq!(extensions(&declined)[CODE_EXTENSION], "PAYMENT_DECLINED");

        let logs = nest_rs_testing::LogCapture::install();
        let ledger = ErrorReport(Payment::Ledger(Ledger))
            .into_failure()
            .into_answer();
        assert_eq!(ledger.message, OPAQUE_CLIENT_MESSAGE);
        let event = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert_eq!(event.level, "error");
        assert!(
            event.field("error").is_some_and(|e| e.contains("10.0.0.1")),
            "{event:#?}"
        );
    }

    #[derive(Debug)]
    struct Outage(Ledger);

    impl std::fmt::Display for Outage {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the payment provider is down")
        }
    }

    impl StdError for Outage {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(&self.0)
        }
    }

    impl ToProblem for Outage {
        fn to_problem(&self) -> Option<Problem> {
            Some(Problem::new(503, code::UNAVAILABLE).with_retry_after(30))
        }
    }

    /// A server problem answers the sentence an opaque failure answers, so it
    /// files the chain it withholds — at `warn` for a `503`, the server unable
    /// to serve in time, at `error` otherwise; a bare problem or a client one
    /// withholds nothing and files none.
    #[test]
    fn a_server_problem_files_the_chain_its_answer_withholds() {
        use super::ErrorReportChain as _;

        let logs = nest_rs_testing::LogCapture::install();
        let failed = || logs.find(crate::TARGET, "graphql operation failed");

        let outage = ErrorReport(Outage(Ledger)).into_failure().into_answer();
        assert_eq!(outage.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(extensions(&outage)[CODE_EXTENSION], "UNAVAILABLE");
        let unavailable = logs.expect_one(crate::TARGET, "graphql operation unavailable");
        assert_eq!(unavailable.level, "warn");
        assert!(
            unavailable
                .field("error")
                .is_some_and(|e| e.contains("10.0.0.1")),
            "{unavailable:#?}"
        );
        assert!(failed().is_empty(), "{:#?}", failed());

        let carried = nest_rs_core::anyhow::Error::from(Problem::new(500, code::INTERNAL))
            .context("charging the card");
        let charged = ErrorReport(carried).into_failure().into_answer();
        assert_eq!(charged.message, OPAQUE_CLIENT_MESSAGE);
        let lines = failed();
        assert_eq!(lines.len(), 1, "{lines:#?}");
        assert_eq!(lines[0].level, "error");
        assert!(
            lines[0]
                .field("error")
                .is_some_and(|e| e.contains("charging the card")),
            "{lines:#?}"
        );

        let bare = problem_error(&Problem::new(503, code::UNAVAILABLE));
        assert_eq!(bare.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(failed().len(), 1, "a bare problem withholds nothing");

        let conflict = problem_error(&Problem::new(409, code::CONFLICT));
        assert_eq!(conflict.message, "conflict");
        assert_eq!(failed().len(), 1, "a client problem withholds nothing");
    }

    #[test]
    fn the_vocabulary_a_chain_carries_is_answered_and_anything_else_is_opaque() {
        use super::ErrorReportChain as _;

        let wrapped = nest_rs_core::anyhow::Error::from(Problem::new(404, code::NOT_FOUND))
            .context("loading the post");
        let found = ErrorReport(wrapped).into_failure().into_answer();
        assert_eq!(found.message, "not found");
        assert_eq!(extensions(&found)[CODE_EXTENSION], "NOT_FOUND");

        let piped = ErrorReport(PipeError::new("must be a valid UUID"))
            .into_failure()
            .into_answer();
        assert_eq!(piped.message, "must be a valid UUID");

        let decode = serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        let said = ErrorReport(decode).into_failure().into_answer();
        assert!(!said.message.contains("sk_live"), "{}", said.message);
        assert_eq!(extensions(&said)[CODE_EXTENSION], "INVALID_ARGUMENT");

        let logs = nest_rs_testing::LogCapture::install();
        let secret = ErrorReport(std::io::Error::other("secret 42"))
            .into_failure()
            .into_answer();
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

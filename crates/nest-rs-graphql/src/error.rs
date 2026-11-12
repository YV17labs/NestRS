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
//! gets, naming the root field it was met under by `kind` and `field`. An
//! error built where no site answers it — `.opaque()` or [`problem_error`] in a
//! body, a resolver written by hand — keeps its chain as its `source`, which
//! its `ServerError` carries into the response: a site takes it there, and what
//! no site answered is filed from the response's errors where its operation or
//! a subscription's item answers, or from the errors it reported where it ends
//! unanswered ([`file_kept`]).
//!
//! An error that is none of these — a `Display` type that is no
//! `std::error::Error` — does not compile.
//!
//! [`OPAQUE_CLIENT_MESSAGE`]: nest_rs_core::OPAQUE_CLIENT_MESSAGE

use std::error::Error as StdError;
use std::sync::{Arc, Mutex, PoisonError};

use async_graphql::{Error, ErrorExtensions, PathSegment, ServerError};
use nest_rs_core::__private::{Withheld, caught_problem};
use nest_rs_core::{DecodeError, Failure, Problem, ServedAddress, ToProblem, code};
use nest_rs_guards::Denial;
use nest_rs_pipes::PipeError;

use crate::redact::unheld_data;

/// The extension member carrying field-level validation errors — the name
/// every other transport uses for them.
pub const FIELD_ERRORS_EXTENSION: &str = "errors";

/// The extension member carrying the [`Code`](nest_rs_core::Code) a client
/// branches on.
pub const CODE_EXTENSION: &str = "code";

/// What an error rendered from a `5xx` carries as its source, a member no
/// response serializes: the failure is the server's own, so its unit's span
/// records it, where one the caller's request earned fails nothing; and the
/// chain the answer keeps from its client, until the edge files it.
#[derive(Debug, Default)]
struct ServerFailed {
    withheld: Mutex<Option<Withheld>>,
}

impl ServerFailed {
    /// The chain kept, taken once: every clone of the error shares it.
    fn take(&self) -> Option<Withheld> {
        self.withheld
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// `error` answering `status`, marked when that is a failure of the server's own.
fn answering(mut error: Error, status: u16) -> Error {
    if status >= 500 {
        error.source = Some(Arc::new(ServerFailed::default()));
    }
    error
}

/// The chain `error` keeps from its client for the edge, taken for filing.
pub(crate) fn take_withheld(error: &Error) -> Option<Withheld> {
    error
        .source
        .as_ref()?
        .downcast_ref::<ServerFailed>()?
        .take()
}

/// File the chain each of `errors` still keeps — an error no site answered —
/// naming the root field it was met under in an operation of `kind`, the
/// field read off `roots` by the key its path opens with: what its operation
/// or a subscription's item answers once nothing else can.
pub(crate) fn file_kept(
    errors: &[ServerError],
    kind: Option<&'static str>,
    roots: &crate::root_fields::RootFields,
) {
    for error in errors {
        let Some(withheld) = error.source::<ServerFailed>().and_then(ServerFailed::take) else {
            continue;
        };
        let mut at = ServedAddress::default();
        at.kind = kind;
        at.field = error
            .path
            .iter()
            .find_map(|segment| match segment {
                PathSegment::Field(key) => Some(key.as_str()),
                PathSegment::Index(_) => None,
            })
            .and_then(|key| roots.site(key))
            .map(|site| site.handler().address());
        file(withheld, at);
    }
}

/// An error a GraphQL unit answers with, read for what its span records.
pub(crate) trait AnsweredError {
    /// Whether it answers a failure of the server's own, a `5xx`'s class, a
    /// type its context does not hold among them, which its response answers
    /// opaquely. Any other — a client's class, the edge's own error as a body
    /// built it, what validation refused — is one the caller's request earned.
    fn failed_the_server(&self) -> bool;
}

impl AnsweredError for Error {
    fn failed_the_server(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.is::<ServerFailed>())
            || unheld_data(&self.message).is_some()
    }
}

impl AnsweredError for ServerError {
    fn failed_the_server(&self) -> bool {
        self.source::<ServerFailed>().is_some() || unheld_data(&self.message).is_some()
    }
}

/// Render a [`PipeError`] as an `async_graphql::Error`: its message, the
/// `INVALID_ARGUMENT` code HTTP's `400` names under `extensions.code`, and any
/// structured field errors under `extensions.errors`.
pub fn pipe_error(err: &PipeError) -> Error {
    // Absent, not null: a client branches on presence.
    let details = err.details().and_then(field_map);
    Error::new(err.message()).extend_with(move |_, e| {
        e.set(CODE_EXTENSION, code::INVALID_ARGUMENT.as_str());
        if let Some(details) = details {
            e.set(FIELD_ERRORS_EXTENSION, details);
        }
    })
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
    let error = Error::new(client_message(&denial)).extend_with(move |_, e| {
        e.set(CODE_EXTENSION, code);
        if let Some(required) = required {
            e.set("requiredScopes", required);
        }
        if let Some(retry_after) = retry_after {
            e.set("retryAfterSeconds", retry_after);
        }
    });
    answering(error, denial.http_status())
}

/// Render what a [`ToProblem`] error says — a [`Problem`] among them — as an
/// `async_graphql::Error`: the problem's
/// [`client_message`](Problem::client_message), its code under
/// `extensions.code`, its RFC 9457 type and title under `extensions.type` and
/// `extensions.title`, its wait under `extensions.retryAfterSeconds` and its
/// field map under `extensions.errors`, each when it has one; or,
/// when the error says none, the opaque answer. Either way the chain an answer
/// withholds — the opaque one's, a `5xx` problem's — rides the error as its
/// `source`, which the edge files at `error` once the answer is final, naming
/// the root field.
///
/// A resolver returning the error answers through it already; a body mixing
/// failures inside an `async_graphql::Result` converts with it.
///
/// ```
/// use nest_rs_core::{Problem, code};
///
/// let error = nest_rs_graphql::problem_error(&Problem::new(409, code::CONFLICT));
/// assert_eq!(error.message, "conflict");
/// ```
pub fn problem_error<E: ToProblem>(error: &E) -> Error {
    problem_reply(error).kept()
}

/// A field map as an extension's value. Plain JSON always converts; a failure
/// would drop the field map, not the error.
fn field_map(errors: &serde_json::Value) -> Option<async_graphql::Value> {
    async_graphql::Value::from_json(errors.clone()).ok()
}

/// A problem in GraphQL's form: its code, its RFC 9457 `type` and `title`
/// when it names them, its wait and its field map, each an extension.
pub(crate) fn rendered(problem: &Problem) -> Error {
    let code = problem.code().as_str();
    let problem_type = problem.type_uri().zip(problem.title());
    let retry_after = problem.retry_after();
    let errors = problem.errors().and_then(field_map);
    let error = Error::new(problem.client_message()).extend_with(move |_, e| {
        e.set(CODE_EXTENSION, code);
        if let Some((type_uri, title)) = problem_type {
            e.set("type", type_uri);
            e.set("title", title);
        }
        if let Some(seconds) = retry_after {
            e.set("retryAfterSeconds", seconds);
        }
        if let Some(errors) = errors {
            e.set(FIELD_ERRORS_EXTENSION, errors);
        }
    });
    answering(error, problem.status())
}

/// What the request sent, refused: `detail`, under the `INVALID_ARGUMENT` code.
pub(crate) fn invalid_argument(detail: String) -> Error {
    rendered(&Problem::new(400, code::INVALID_ARGUMENT).with_detail(detail))
}

/// The opaque answer, keeping `error`'s whole chain for the operator's line:
/// what a failure the client is owed no explanation for becomes.
pub(crate) fn opaque_error(error: &(dyn StdError + 'static)) -> Error {
    Reply::opaque(error).kept()
}

/// The operator's line for the chain an answer kept from its client, naming the
/// root field `at` as its `graphql.field` line does, by `kind` and `field`: a
/// failure at `error`, a `503` — the server could not serve it in time, which
/// is no failure of its own — at `warn`.
fn file(withheld: Withheld, at: ServedAddress<'_>) {
    match withheld {
        Withheld::Failed(said) => tracing::error!(
            target: crate::TARGET,
            kind = at.kind,
            field = at.field,
            error = %said,
            "graphql operation failed",
        ),
        Withheld::Unavailable(said) => tracing::warn!(
            target: crate::TARGET,
            kind = at.kind,
            field = at.field,
            error = %said,
            "graphql operation unavailable",
        ),
    }
}

/// Tiers 3 and 4: the framework vocabulary `error`'s chain carries, else the
/// opaque answer.
fn chain_error(error: &(dyn StdError + 'static)) -> Reply {
    use nest_rs_core::__private::find_in_chain;

    if let Some(problem) = find_in_chain::<Problem>(error) {
        return Reply::problem(problem, error, None);
    }
    if let Some(rejection) = find_in_chain::<PipeError>(error) {
        return Reply::said(pipe_error(rejection));
    }
    if let Some(report) = DecodeError::in_chain(error) {
        return Reply::said(invalid_argument(report.to_string()));
    }
    Reply::opaque(error)
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
    /// `error`, an answer that keeps nothing from its client.
    fn said(error: Error) -> Self {
        Self {
            error,
            withheld: None,
        }
    }

    /// `problem` as the answer to `error`, keeping what a `5xx` answer
    /// withholds: the chain the failure kept of its own (`kept`, `.opaque()`'s)
    /// at that `5xx`'s level, else `error`'s.
    fn problem(
        problem: &Problem,
        error: &(dyn StdError + 'static),
        kept: Option<Withheld>,
    ) -> Self {
        Self {
            error: rendered(problem),
            withheld: nest_rs_core::__private::withheld_over(problem.status(), error, kept),
        }
    }

    /// The opaque answer, keeping `error`'s whole chain.
    pub(crate) fn opaque(error: &(dyn StdError + 'static)) -> Self {
        Self {
            // The `INTERNAL` code an internal denial carries, so the two are
            // indistinguishable.
            error: rendered(&Problem::new(500, code::INTERNAL)),
            withheld: Some(Withheld::Failed(nest_rs_core::error_message(error))),
        }
    }

    /// The answer, its withheld chain filed naming the root field `at`: it is
    /// the one sent.
    pub(crate) fn filed_at(self, at: ServedAddress<'_>) -> Error {
        if let Some(withheld) = self.withheld {
            file(withheld, at);
        }
        self.error
    }

    /// The answer, its withheld chain riding it as its `source`, for the edge
    /// to file once no site can answer it otherwise.
    fn kept(self) -> Error {
        let Self {
            mut error,
            withheld,
        } = self;
        if let Some(withheld) = withheld {
            error.source = Some(Arc::new(ServerFailed {
                withheld: Mutex::new(Some(withheld)),
            }));
        }
        error
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
    /// answers it, the chain it withholds kept for the edge to file.
    pub fn into_answer(self) -> Error {
        match self.reply {
            Some(reply) => reply.kept(),
            None => unanswered(&self.failure).kept(),
        }
    }
}

/// The edge's own error, as built: its own answer, the chain it keeps
/// (`.opaque()`'s) the site's to file.
impl From<Error> for FieldFailure {
    fn from(error: Error) -> Self {
        let withheld = take_withheld(&error);
        Self {
            failure: Failure::from(DeliberateError),
            reply: Some(Box::new(Reply { error, withheld })),
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

/// A loader a field reads that its request holds no seed of: the type goes on
/// the operator's line, never to the client.
#[derive(Debug, thiserror::Error)]
#[error(
    "`{field}` reads the dataloader `{loader}`, which this request holds no seed of — the module \
     providing it is not imported by (or reachable from) this app"
)]
pub(crate) struct UnseededLoader {
    pub(crate) field: String,
    pub(crate) loader: &'static str,
}

/// What a field read off its `Context` by hand and its request does not hold:
/// the type goes on the operator's line, never to the client.
#[derive(Debug, thiserror::Error)]
#[error(
    "a field read `{data}` off its context, which this request does not hold; a loader read as a \
     `&DataLoader<L>` parameter instead refuses the boot naming the import"
)]
pub(crate) struct UnheldData {
    pub(crate) data: String,
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
/// whatever it wrote — the chain the field's own answer kept (`.opaque()`'s)
/// at the level a `5xx` files it, none behind a `4xx` — else the answer it
/// wrote, else the edge's one rule, each with the chain it keeps from the
/// client.
pub(crate) fn answer(failure: &Failure, reply: Option<Box<Reply>>) -> Reply {
    if let Some(problem) = caught_problem(failure) {
        let kept = reply.and_then(|reply| reply.withheld);
        return Reply::problem(problem, failure.error(), kept);
    }
    match reply {
        Some(reply) if failure.is_answered() => *reply,
        _ => unanswered(failure),
    }
}

/// The edge's one rule for a failure nothing answered: a refusal as its
/// denial, the vocabulary its chain carries, else the opaque answer.
fn unanswered(failure: &Failure) -> Reply {
    match failure.find::<Denial>() {
        Some(denial) => Reply::said(denial_to_graphql_error(denial.clone())),
        None => chain_error(failure.error()),
    }
}

/// The answer to `error`'s [`ToProblem`] — the problem it says, or the opaque
/// answer when it says none — and the chain it keeps from the client.
fn problem_reply<E: ToProblem>(error: &E) -> Reply {
    match error.to_problem() {
        Some(problem) => Reply::problem(&problem, error, None),
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

    /// The chain `err` keeps for the edge to file, said whole.
    fn kept(err: &Error) -> Option<Withheld> {
        take_withheld(err)
    }

    /// A field reads its error before the response answers it opaquely, a
    /// federation field as a `ServerError`: each reads the server's failure.
    #[test]
    fn a_type_a_context_does_not_hold_fails_the_server_before_it_is_redacted() {
        let unheld = Error::new("Data `the_crate::Shelf` does not exist.");
        assert!(unheld.failed_the_server());
        assert!(
            unheld
                .into_server_error(async_graphql::Pos::default())
                .failed_the_server()
        );

        let deliberate = Error::new("Data `the archive` is on the way.");
        assert!(!deliberate.failed_the_server());
        assert!(
            !deliberate
                .into_server_error(async_graphql::Pos::default())
                .failed_the_server()
        );
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
        assert_eq!(
            extensions(&err)[CODE_EXTENSION],
            "INVALID_ARGUMENT",
            "the class HTTP's 400 names, beside the field map",
        );
    }

    #[test]
    fn a_rejection_without_details_carries_its_class_and_no_field_map() {
        let err = pipe_error(&PipeError::new("must be a valid UUID"));
        assert_eq!(err.message, "must be a valid UUID");
        assert_eq!(
            extensions(&err),
            serde_json::json!({ CODE_EXTENSION: "INVALID_ARGUMENT" }),
            "`errors` absent, not null — a client branches on presence",
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
        logs.expect_none(crate::TARGET, "graphql operation failed");
        assert!(
            matches!(kept(&ledger), Some(Withheld::Failed(said)) if said.contains("10.0.0.1")),
            "the answer keeps the chain it withholds for the edge",
        );
    }

    /// A problem's RFC 9457 type and title, and its field map, ride beside its
    /// code in the error's extensions; a problem naming none carries none.
    #[test]
    fn a_typed_problem_carries_its_type_title_and_field_map_as_extensions() {
        let taken = problem_error(
            &Problem::new(409, code::CONFLICT)
                .with_type("https://example.com/problems/handle-taken", "Handle taken")
                .with_detail("@ada is taken")
                .with_errors(serde_json::json!({ "handle": [{ "code": "taken" }] })),
        );
        assert_eq!(taken.message, "@ada is taken");
        assert_eq!(
            extensions(&taken),
            serde_json::json!({
                "code": "CONFLICT",
                "type": "https://example.com/problems/handle-taken",
                "title": "Handle taken",
                FIELD_ERRORS_EXTENSION: { "handle": [{ "code": "taken" }] },
            }),
        );

        let blank = problem_error(&Problem::new(404, code::NOT_FOUND));
        assert_eq!(
            extensions(&blank),
            serde_json::json!({ "code": "NOT_FOUND" })
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
    /// keeps the chain it withholds for the edge — at `warn`'s level for a `503`,
    /// the server unable to serve in time, at `error`'s otherwise; a bare
    /// problem or a client one withholds nothing. Nothing is filed before an
    /// answer is final.
    #[test]
    fn a_server_problem_keeps_the_chain_its_answer_withholds() {
        use super::ErrorReportChain as _;

        let logs = nest_rs_testing::LogCapture::install();

        let outage = ErrorReport(Outage(Ledger)).into_failure().into_answer();
        assert_eq!(outage.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(extensions(&outage)[CODE_EXTENSION], "UNAVAILABLE");
        assert!(
            matches!(kept(&outage), Some(Withheld::Unavailable(said)) if said.contains("10.0.0.1")),
        );

        let carried = nest_rs_core::anyhow::Error::from(Problem::new(500, code::INTERNAL))
            .context("charging the card");
        let charged = ErrorReport(carried).into_failure().into_answer();
        assert_eq!(charged.message, OPAQUE_CLIENT_MESSAGE);
        assert!(
            matches!(kept(&charged), Some(Withheld::Failed(said)) if said.contains("charging the card")),
        );
        assert_eq!(kept(&charged), None, "taken once");

        let bare = problem_error(&Problem::new(503, code::UNAVAILABLE));
        assert_eq!(bare.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(kept(&bare), None, "a bare problem withholds nothing");

        let conflict = problem_error(&Problem::new(409, code::CONFLICT));
        assert_eq!(conflict.message, "conflict");
        assert_eq!(kept(&conflict), None, "a client problem withholds nothing");

        logs.expect_none(crate::TARGET, "graphql operation failed");
        logs.expect_none(crate::TARGET, "graphql operation unavailable");
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

        let secret = ErrorReport(std::io::Error::other("secret 42"))
            .into_failure()
            .into_answer();
        assert_eq!(secret.message, OPAQUE_CLIENT_MESSAGE);
        assert_eq!(extensions(&secret)[CODE_EXTENSION], "INTERNAL");
        assert!(
            matches!(kept(&secret), Some(Withheld::Failed(said)) if said.contains("secret 42")),
        );
    }
}

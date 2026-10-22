//! What a failing operation tells the model, and what it tells the operator.
//!
//! A tool body talks to a **language model**, which may repeat whatever an
//! error's `Display` carries; [`Opaque`] logs the real error and answers an opaque one.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_core::Posture;
//! # use nest_rs_mcp::{DEFAULT_PATH, McpError, McpModule, McpOptions, Opaque, mcp, tools};
//! # use nest_rs_testing::mcp::{call_tool, result};
//! #
//! # #[injectable]
//! # #[derive(Default)]
//! # struct RowsService;
//! #
//! # impl RowsService {
//! #     async fn list(&self) -> Result<Vec<String>, std::io::Error> {
//! #         Err(std::io::Error::other("relation \"rows\" does not exist"))
//! #     }
//! # }
//! #
//! # #[mcp]
//! # struct RowsHost {
//! #     #[inject]
//! #     svc: Arc<RowsService>,
//! # }
//! #
//! # #[tools]
//! # impl RowsHost {
//! #     #[tool(description = "List the rows.")]
//! #     #[public]
//! #     async fn list_rows(&self) -> Result<String, McpError> {
//! let rows = self.svc.list().await.opaque()?;
//! #         Ok(rows.join("\n"))
//! #     }
//! # }
//! #
//! # #[module(imports = [McpModule::for_root(McpOptions { posture: Posture::Public, ..Default::default() })], providers = [RowsService, RowsHost])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
//! # let body = call_tool(app.http(), DEFAULT_PATH, "list_rows", None).await;
//!
//! assert_eq!(result(&body)["error"]["message"], nest_rs_core::OPAQUE_CLIENT_MESSAGE);
//! # Ok(())
//! # }
//! ```
//!
//! A deliberate `McpError::invalid_params(…)`, meant for the model to read and
//! retry, is returned directly, never through here.

use std::error::Error as StdError;

use nest_rs_core::__private::{Withheld, caught_problem, find_in_chain};
use nest_rs_core::{DecodeError, Failure, Problem, ToProblem};
use nest_rs_guards::Denial;
use nest_rs_pipes::PipeError;
use rmcp::ErrorData as McpError;
use rmcp::model::ErrorCode;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManagerError, SessionError,
};
use rmcp::transport::streamable_http_server::session::store::SessionStoreError;

use nest_rs_core::OPAQUE_CLIENT_MESSAGE as OPAQUE;

/// Turn a failure the model must not read into one it may.
///
/// Implemented for every `Result` whose error converts into a boxed error; the
/// whole cause chain is logged at `error` on `nest_rs::mcp`.
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the model an opaque one.
    fn opaque(self) -> Result<T, McpError>;
}

impl<T, E> Opaque<T> for Result<T, E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    fn opaque(self) -> Result<T, McpError> {
        self.map_err(|err| opaque_error(&*nest_rs_core::boxed_error(err)))
    }
}

/// Render a rejected pipe as an `invalid_params` the model can act on, its
/// `details` as the error's data; emitted by `#[tools]`, never written by hand.
pub fn pipe_error(err: &PipeError) -> McpError {
    McpError::invalid_params(err.message().to_owned(), err.details().cloned())
}

/// Render what a [`ToProblem`] error says — a
/// [`Problem`] among them — as the JSON-RPC error an MCP
/// operation answers with: a `4xx` as `invalid_request`, a `5xx` as
/// `internal_error`, the problem's
/// [`client_message`](nest_rs_core::Problem::client_message) as the message,
/// and its code —
/// with its wait as `retryAfterSeconds` — under `data`. An error that says none
/// answers as [`Opaque`] does, its chain filed at `error`, as is the chain
/// behind a `5xx` problem.
///
/// ```
/// use nest_rs_core::Problem;
/// use nest_rs_core::problem::code;
///
/// let error = nest_rs_mcp::problem_error(&Problem::new(404, code::NOT_FOUND));
/// assert_eq!(error.message, "not found");
/// assert_eq!(error.data, Some(serde_json::json!({ "code": "NOT_FOUND" })));
/// ```
pub fn problem_error<E: ToProblem>(error: &E) -> McpError {
    problem_reply(error).filed()
}

/// A problem as the JSON-RPC error an MCP operation answers with.
fn rendered(problem: &Problem) -> McpError {
    let mut data = serde_json::Map::new();
    data.insert("code".to_owned(), problem.code().as_str().into());
    if let Some(seconds) = problem.retry_after() {
        data.insert("retryAfterSeconds".to_owned(), seconds.into());
    }
    let data = Some(serde_json::Value::Object(data));
    if problem.is_server_error() {
        McpError::internal_error(problem.client_message(), data)
    } else {
        McpError::invalid_request(problem.client_message(), data)
    }
}

/// `problem` rendered, the chain of `error` behind it filed when its answer
/// withholds it.
fn answered(problem: &Problem, error: &(dyn StdError + 'static)) -> McpError {
    file(nest_rs_core::__private::withheld(problem, error));
    rendered(problem)
}

/// The operator's line for a chain an answer withholds, at its level.
fn file(withheld: Option<Withheld>) {
    match withheld {
        Some(Withheld::Failed(said)) => failed(&said),
        Some(Withheld::Unavailable(said)) => unavailable(&said),
        None => {}
    }
}

/// The opaque answer, with `error`'s whole chain on the operator's line.
fn opaque_error(error: &(dyn StdError + 'static)) -> McpError {
    failed(&nest_rs_core::error_message(error));
    opaque_answer()
}

/// The opaque answer alone, for a failure whose cause is filed elsewhere.
pub(crate) fn opaque_answer() -> McpError {
    McpError::internal_error(OPAQUE, None)
}

/// The vocabulary `error`'s chain carries, else the opaque answer: the
/// method's own error, a problem, a pipe's refusal or a decode failure,
/// first found from the error itself down.
fn chain_error(error: &(dyn StdError + 'static)) -> McpError {
    if let Some(deliberate) = find_in_chain::<McpError>(error) {
        return deliberate.clone();
    }
    if let Some(problem) = find_in_chain::<Problem>(error) {
        return answered(problem, error);
    }
    if let Some(rejection) = find_in_chain::<PipeError>(error) {
        return pipe_error(rejection);
    }
    if let Some(report) = DecodeError::in_chain(error) {
        return McpError::invalid_params(report.to_string(), None);
    }
    opaque_error(error)
}

/// Why an operation's tail failed: the failure the pipeline carries, which
/// its exception filters read, and the answer only the error's type could say,
/// held until the operation's unit decides whether it goes out.
pub struct OperationFailure {
    pub(crate) failure: Failure,
    pub(crate) reply: Option<Box<Reply>>,
}

/// An answer an operation wrote, with the chain it keeps from the client —
/// filed only if it is the one sent.
pub(crate) struct Reply {
    error: McpError,
    withheld: Option<Withheld>,
}

impl Reply {
    /// The answer, its withheld chain filed: it is the one sent.
    pub(crate) fn filed(self) -> McpError {
        file(self.withheld);
        self.error
    }
}

impl OperationFailure {
    /// A failure the edge's one rule answers once the unit ran.
    pub(crate) fn unanswered(failure: Failure) -> Self {
        Self {
            failure,
            reply: None,
        }
    }

    /// What an operation failing with this answers when no exception filter
    /// answers it, the chain it withholds filed.
    #[cfg(test)]
    pub(crate) fn into_answer(self) -> McpError {
        match self.reply {
            Some(reply) => reply.filed(),
            None => unanswered(&self.failure),
        }
    }
}

/// The operation's own `McpError`, as built: its own answer, which a filter
/// still reads.
impl From<McpError> for OperationFailure {
    fn from(error: McpError) -> Self {
        Self {
            failure: Failure::from(error.clone()),
            reply: Some(Box::new(Reply {
                error,
                withheld: None,
            })),
        }
    }
}

/// A per-argument pipe's refusal, answered as one.
impl From<PipeError> for OperationFailure {
    fn from(refused: PipeError) -> Self {
        Self::unanswered(Failure::from(refused))
    }
}

/// What an operation whose unit failed answers: an exception filter's problem
/// over whatever it wrote, else the answer it wrote, else the edge's one rule
/// — each filing the chain it keeps from the client.
pub(crate) fn answer(failure: &Failure, reply: Option<Box<Reply>>) -> McpError {
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
fn unanswered(failure: &Failure) -> McpError {
    match failure.find::<Denial>() {
        Some(denial) => crate::guard::denial_to_mcp_error(denial.clone()),
        None => chain_error(failure.error()),
    }
}

/// The answer to `error`'s [`ToProblem`] — the problem it says, or the opaque
/// answer when it says none — and the chain it keeps from the client.
fn problem_reply<E: ToProblem>(error: &E) -> Reply {
    match error.to_problem() {
        Some(problem) => Reply {
            error: rendered(&problem),
            withheld: nest_rs_core::__private::withheld(&problem, error),
        },
        None => Reply {
            error: opaque_answer(),
            withheld: Some(Withheld::Failed(nest_rs_core::error_message(error))),
        },
    }
}

/// An operation's error, rendered by its type: the inherent method takes a
/// [`ToProblem`] error, [`ErrorReportChain`] any other — the operation's own
/// `McpError` as built, else the vocabulary its chain carries, else opaque.
pub struct ErrorReport<E>(pub E);

impl<E: ToProblem> ErrorReport<E> {
    /// The problem the error says, or the opaque answer; the error stays
    /// typed for the exception filters.
    pub fn into_failure(self) -> OperationFailure {
        let reply = Box::new(problem_reply(&self.0));
        OperationFailure {
            failure: Failure::from(self.0),
            reply: Some(reply),
        }
    }
}

/// Every error [`ErrorReport`]'s inherent method does not take.
pub trait ErrorReportChain {
    /// The operation's own `McpError` as built, else a failure the edge's one
    /// rule answers.
    fn into_failure(self) -> OperationFailure;
}

impl<E> ErrorReportChain for ErrorReport<E>
where
    E: Into<Box<dyn StdError + Send + Sync>> + 'static,
{
    fn into_failure(self) -> OperationFailure {
        let boxed = nest_rs_core::boxed_error(self.0);
        match boxed.downcast::<McpError>() {
            Ok(deliberate) => OperationFailure::from(*deliberate),
            Err(other) => OperationFailure::unanswered(Failure::new(other)),
        }
    }
}

/// The operator's line for a failure the answer withholds: its whole chain.
fn failed(said: &str) {
    tracing::error!(
        target: crate::TARGET,
        error = %said,
        "mcp operation failed",
    );
}

/// The operator's line for a `503` the answer withholds the cause of: the
/// server could not serve it in time, which is no failure of its own.
fn unavailable(said: &str) {
    tracing::warn!(
        target: crate::TARGET,
        error = %said,
        "mcp operation unavailable",
    );
}

/// Whether `error` is rmcp's `PromptRouter` refusing `name`, a prompt its host
/// does not declare, in words quoting the name a client sent.
pub(crate) fn refuses_prompt(error: &McpError, name: &str) -> bool {
    error.code == ErrorCode::INVALID_PARAMS
        && error
            .message
            .strip_prefix("prompt '")
            .and_then(|rest| rest.strip_suffix("' not found"))
            == Some(name)
}

/// The key under which rmcp's refusal of a prompt lists the prompts its host
/// declares.
pub(crate) const AVAILABLE_PROMPTS: &str = "available_prompts";

/// The refusal of a prompt no host serves, said by kind, its `data` the
/// prompts the hosts declare as rmcp lists them.
pub(crate) fn prompt_not_found(data: Option<serde_json::Value>) -> McpError {
    McpError::invalid_params("prompt not found", data)
}

/// Whether `error` is rmcp's `TaskManager` refusing `id`, a task it does not
/// hold, in words quoting the id a client sent.
pub(crate) fn refuses_task(error: &McpError, id: &str) -> bool {
    error.code == ErrorCode::INVALID_PARAMS
        && error.message.strip_prefix(UNKNOWN_TASK_QUOTED) == Some(id)
}

/// How rmcp's `TaskManager` opens its refusal of a task, the id following it.
const UNKNOWN_TASK_QUOTED: &str = "unknown task: ";

/// The refusal of a task no host holds, said by kind.
pub(crate) fn task_not_found() -> McpError {
    McpError::invalid_params("unknown task", None)
}

/// Whether `error` is rmcp's default answer to a custom method no host serves,
/// whose message is the method as a client sent it.
pub(crate) fn refuses_method(error: &McpError, method: &str) -> bool {
    error.code == ErrorCode::METHOD_NOT_FOUND && error.message == method
}

/// The refusal of a method no host serves, in JSON-RPC 2.0's words (§5.1).
pub(crate) fn method_not_found() -> McpError {
    McpError::new(ErrorCode::METHOD_NOT_FOUND, METHOD_NOT_FOUND, None)
}

/// JSON-RPC 2.0's words for an unknown method (§5.1).
const METHOD_NOT_FOUND: &str = "Method not found";

/// What a JSON-RPC error `code` says: JSON-RPC 2.0's words for its own codes
/// (§5.1) and MCP's for the codes rmcp names; `None` for any other.
pub(crate) fn said_by_code(code: ErrorCode) -> Option<&'static str> {
    Some(match code {
        ErrorCode::PARSE_ERROR => "Parse error",
        ErrorCode::INVALID_REQUEST => "Invalid Request",
        ErrorCode::METHOD_NOT_FOUND => METHOD_NOT_FOUND,
        ErrorCode::INVALID_PARAMS => "Invalid params",
        ErrorCode::INTERNAL_ERROR => "Internal error",
        ErrorCode::RESOURCE_NOT_FOUND => "Resource not found",
        ErrorCode::HEADER_MISMATCH => "Header mismatch",
        ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY => "Missing required client capability",
        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION => "Unsupported protocol version",
        _ => return None,
    })
}

/// rmcp's refusal of a tool no router declares, a `-32602` read as *not
/// found*.
pub(crate) const TOOL_NOT_FOUND: &str = "tool not found";

/// The refusal of a tool no host declares, in rmcp's own words.
pub(crate) fn tool_not_found() -> McpError {
    McpError::invalid_params(TOOL_NOT_FOUND, None)
}

/// rmcp 3.5's answer to a session it does not hold, word for word.
pub(crate) const SESSION_NOT_FOUND: &str = "Not Found: Session not found";

/// rmcp 3.5's streamable-HTTP refusals written as text, word for word, that
/// quote nothing the request carried: what a client needs to fix its call.
const TRANSPORT_SENTENCES: [&str; 15] = [
    "Not Acceptable: Client must accept both application/json and text/event-stream",
    "Not Acceptable: Client must accept text/event-stream",
    "Unsupported Media Type: Content-Type must be application/json",
    "Bad Request: Session ID is required",
    SESSION_NOT_FOUND,
    "Bad Request: Invalid MCP-Protocol-Version header encoding",
    "Bad Request: Invalid Host header encoding",
    "Bad Request: Invalid Host header",
    "Bad Request: missing Host header",
    "Forbidden: Host header is not allowed",
    "Forbidden: Invalid Origin header encoding",
    "Forbidden: Invalid Origin header",
    "Forbidden: Origin header is not allowed",
    "Method Not Allowed",
    "Unexpected message, expect initialize request",
];

/// rmcp's refusal of a body past the cap, the configured cap between the two.
const TOO_LARGE: (&str, &str) = ("Payload Too Large: request body exceeds ", " bytes");

/// rmcp 3.5's JSON-RPC refusals, word for word, that quote nothing the request
/// carried.
const TRANSPORT_MESSAGES: [&str; 7] = [
    "Invalid Request: MCP-Protocol-Version header is not valid UTF-8",
    "request _meta protocolVersion requires MCP-Protocol-Version header",
    "Missing MCP-Protocol-Version header for request requiring per-request protocol metadata",
    "Invalid params: request requires protocolVersion in request _meta",
    "multiple Mcp-Method header values supplied; initialize requires exactly one",
    "missing required Mcp-Method header",
    "Mcp-Name header is not valid Base64",
];

/// rmcp's refusal of a request missing `_meta` keys, the keys it lists after.
const META_MISSING: &str =
    "Invalid params: request _meta is missing or has malformed required fields: ";

/// The `_meta` keys MCP requires on a request from `2026-07-28`, as the
/// specification names them.
const REQUIRED_META_KEYS: [&str; 2] = [
    "io.modelcontextprotocol/protocolVersion",
    "io.modelcontextprotocol/clientCapabilities",
];

/// `text`, when it is one of rmcp's transport refusals quoting nothing the
/// request carried.
pub(crate) fn transport_sentence(text: &str) -> Option<&str> {
    if TRANSPORT_SENTENCES.contains(&text) {
        return Some(text);
    }
    let (opening, closing) = TOO_LARGE;
    let cap = text.strip_prefix(opening)?.strip_suffix(closing)?;
    (!cap.is_empty() && cap.bytes().all(|byte| byte.is_ascii_digit())).then_some(text)
}

/// Whether `message` is one of rmcp's JSON-RPC transport refusals quoting
/// nothing the request carried: a constant, or the `_meta` keys it lists when
/// each is one the specification fixes.
pub(crate) fn is_transport_message(message: &str) -> bool {
    TRANSPORT_MESSAGES.contains(&message)
        || message.strip_prefix(META_MISSING).is_some_and(|keys| {
            !keys.is_empty()
                && keys
                    .split(", ")
                    .all(|key| REQUIRED_META_KEYS.contains(&key))
        })
}

/// What an MCP operation answers: a `Result`, whatever the type is called.
#[diagnostic::on_unimplemented(
    message = "an MCP operation returns a `Result`, and `{Self}` is not one",
    label = "not a `Result`",
    note = "its guards, its access posture and its pipes refuse through the operation's \
            failure, and an operation that cannot fail has nowhere to report a denial"
)]
pub trait OperationAnswer {
    /// What the operation answers when it succeeds.
    type Value;
    /// What it fails with: an `McpError`, a `ToProblem` error or any other
    /// error, rendered by its type.
    type Error;

    /// The answer as the `Result` it is.
    fn into_result(self) -> Result<Self::Value, Self::Error>;
}

impl<T, E> OperationAnswer for Result<T, E> {
    type Value = T;
    type Error = E;

    fn into_result(self) -> Self {
        self
    }
}

/// The operation's answer as the `Result` it is — see [`OperationAnswer`].
pub fn answer_of<R, V, E>(answer: R) -> Result<V, E>
where
    R: OperationAnswer<Value = V, Error = E>,
{
    answer.into_result()
}

/// What a tool call whose result reports its error in band fails its unit
/// with: the answer is the result itself.
#[derive(Debug, thiserror::Error)]
#[error("the tool reported its error in its result")]
pub(crate) struct ToolReportedError;

/// Why an operation's `Scoped<T>` parameter resolved nothing — said to the
/// operator alone, the client answered opaquely.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ScopeError {
    /// No request scope reached the operation.
    #[error("no request scope reaches the operation to resolve `{provider}`")]
    NotInstalled {
        /// The provider the parameter names.
        provider: &'static str,
    },
    /// The scope's container offers no such provider.
    #[error("no provider registered for `{provider}` — add it to a module's providers")]
    NoProvider {
        /// The provider the parameter names.
        provider: &'static str,
    },
}

/// Why a mount's session manager refused: its cap, the principal a session is
/// bound to, the app's store, or rmcp's own manager.
///
/// rmcp writes the `Display` into the `500` it answers a refused session with
/// — which the endpoint answers as the cap's `503` for [`Full`](Self::Full),
/// and as a session it does not hold for [`Unknown`](Self::Unknown) — so no
/// variant names the session.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionsError {
    /// The mount holds its `max_concurrent_sessions`.
    #[error("the mount holds its maximum of live sessions")]
    Full,
    /// The session is another principal's, or one the mount does not hold.
    #[error("no session of the caller's")]
    Unknown,
    /// The app's session store failed to say whose a session is.
    #[error("the session store failed")]
    Store(#[source] SessionStoreError),
    /// rmcp's local session manager refused.
    #[error("the session manager failed")]
    Local(#[from] LocalSessionManagerError),
}

nest_rs_core::inventory::submit! { nest_rs_core::SafeLink::new(say_session) }

/// rmcp's session manager error said by its kind: its own sentence names the
/// session a client named.
fn say_session(link: &(dyn std::error::Error + 'static)) -> Option<String> {
    let said = match link.downcast_ref::<LocalSessionManagerError>()? {
        LocalSessionManagerError::SessionNotFound(_) => "session not found",
        LocalSessionManagerError::SessionError(error) => match error {
            SessionError::DuplicatedRequestId(_) => "a request id the session already holds",
            SessionError::ChannelClosed(_) => "the session's channel closed",
            SessionError::EventIdParseError(_) => "an event id that does not parse",
            SessionError::SessionServiceTerminated => "the session's service terminated",
            SessionError::InvalidEventId => "an event id the session does not hold",
            SessionError::Io(error) => {
                return Some(format!("session I/O failed: {}", error.kind()));
            }
            SessionError::EventStore(_) => "the event store failed",
            _ => "the session failed",
        },
        LocalSessionManagerError::InvalidEventId(_) => "an event id that does not parse",
        _ => "the session manager failed",
    };
    Some(said.to_owned())
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Problem;

    use super::*;

    /// rmcp's sentences quoting nothing are kept word for word; one quoting
    /// what the request carried, or reworded, is not.
    #[test]
    fn a_transport_refusal_is_kept_only_in_rmcp_s_words_quoting_nothing() {
        for kept in TRANSPORT_SENTENCES {
            assert_eq!(transport_sentence(kept), Some(kept));
        }
        let too_large = "Payload Too Large: request body exceeds 4194304 bytes";
        assert_eq!(transport_sentence(too_large), Some(too_large));
        for refused in [
            "Payload Too Large: request body exceeds marked bytes",
            "Payload Too Large: request body exceeds  bytes",
            "Bad Request: Unsupported MCP-Protocol-Version: marked",
            "Not Found: Session not found marked",
            "not found: session not found",
            "",
        ] {
            assert_eq!(transport_sentence(refused), None, "{refused}");
        }

        for kept in TRANSPORT_MESSAGES {
            assert!(is_transport_message(kept), "{kept}");
        }
        assert!(is_transport_message(
            "Invalid params: request _meta is missing or has malformed required fields: \
             io.modelcontextprotocol/clientCapabilities"
        ));
        for refused in [
            "Invalid params: request _meta is missing or has malformed required fields: marked",
            "Invalid params: request _meta is missing or has malformed required fields: \
             io.modelcontextprotocol/protocolVersion, marked",
            "Invalid params: request _meta is missing or has malformed required fields: ",
            "Mcp-Method header `marked` does not match body method `initialize`",
            "missing required Mcp-Name header for `tools/call`",
        ] {
            assert!(!is_transport_message(refused), "{refused}");
        }
    }

    #[test]
    fn a_failure_reaches_the_model_stripped_of_everything_it_said() {
        let leaky: Result<(), String> =
            Err("relation \"users\" does not exist; secret_column = 42".to_owned());

        let err = leaky
            .opaque()
            .expect_err("the failure survives as a failure");
        let rendered = format!("{err:?}");

        assert!(
            !rendered.contains("secret_column") && !rendered.contains("users"),
            "nothing the source error said may reach a language model: {rendered}",
        );
        assert!(
            rendered.contains(OPAQUE),
            "…and what it does say is the one constant message: {rendered}",
        );
    }

    #[test]
    fn a_rejected_pipe_tells_the_model_what_to_fix() {
        let err = pipe_error(&PipeError::with_details(
            "validation failed",
            serde_json::json!({ "file": ["must not be empty"] }),
        ));

        assert_eq!(err.message, "validation failed");
        assert_eq!(
            err.data,
            Some(serde_json::json!({ "file": ["must not be empty"] })),
            "the field errors ride along so the model can correct the argument \
             it got wrong, exactly as the HTTP 400 carries them",
        );
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test reads the line the call logs, not its result"
    )]
    fn and_the_operator_gets_the_error_the_model_does_not() {
        let logs = nest_rs_testing::LogCapture::install();
        let leaky: Result<(), String> =
            Err("relation \"users\" does not exist; secret_column = 42".to_owned());
        let _ = leaky.opaque();

        let event = logs.expect_one("nest_rs::mcp", "mcp operation failed");
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("secret_column")),
            "the cause the reply withholds is exactly what the log has to carry, got {:?}",
            event.fields,
        );
    }

    #[test]
    fn a_problem_answers_its_family_its_sentence_and_its_code() {
        use nest_rs_core::problem::code;

        let refused =
            problem_error(&Problem::new(409, code::CONFLICT).with_detail("the slug is taken"));
        assert_eq!(refused.code, rmcp::model::ErrorCode::INVALID_REQUEST);
        assert_eq!(refused.message, "the slug is taken");
        assert_eq!(
            refused.data,
            Some(serde_json::json!({ "code": "CONFLICT" }))
        );

        let unavailable = problem_error(
            &Problem::new(503, code::UNAVAILABLE)
                .with_detail("store at 10.0.0.1 down")
                .with_retry_after(7),
        );
        assert_eq!(unavailable.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert_eq!(
            unavailable.message,
            nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE
        );
        assert_eq!(
            unavailable.data,
            Some(serde_json::json!({ "code": "UNAVAILABLE", "retryAfterSeconds": 7 })),
        );
    }

    #[test]
    fn a_to_problem_error_saying_none_answers_opaquely_and_files_its_chain() {
        #[derive(Debug)]
        struct LedgerDown;

        impl std::fmt::Display for LedgerDown {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the ledger at 10.0.0.1 refused")
            }
        }

        impl std::error::Error for LedgerDown {}

        impl ToProblem for LedgerDown {
            fn to_problem(&self) -> Option<Problem> {
                None
            }
        }

        let logs = nest_rs_testing::LogCapture::install();
        let error = problem_error(&LedgerDown);
        assert_eq!(error.message, OPAQUE);
        assert!(!format!("{error:?}").contains("10.0.0.1"), "{error:?}");
        let failed = logs.expect_one("nest_rs::mcp", "mcp operation failed");
        assert!(
            failed
                .field("error")
                .is_some_and(|e| e.contains("10.0.0.1"))
        );
    }

    /// A server problem answers the sentence an opaque failure answers, so it
    /// files the line an opaque failure files; a bare problem or a client one
    /// withholds nothing and files none.
    #[test]
    fn a_server_problem_files_the_chain_its_answer_withholds() {
        use nest_rs_core::problem::code;

        #[derive(Debug)]
        struct Outage;

        impl std::fmt::Display for Outage {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the search index at 10.0.0.1 refused")
            }
        }

        impl std::error::Error for Outage {}

        impl ToProblem for Outage {
            fn to_problem(&self) -> Option<Problem> {
                Some(Problem::new(503, code::UNAVAILABLE))
            }
        }

        let logs = nest_rs_testing::LogCapture::install();
        let error = problem_error(&Outage);
        assert_eq!(error.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert!(!format!("{error:?}").contains("10.0.0.1"), "{error:?}");
        let unavailable = logs.expect_one("nest_rs::mcp", "mcp operation unavailable");
        assert_eq!(
            unavailable.level, "warn",
            "the server could not serve it in time"
        );
        assert!(
            unavailable
                .field("error")
                .is_some_and(|e| e.contains("10.0.0.1")),
            "{unavailable:#?}"
        );

        let bare = problem_error(&Problem::new(503, code::UNAVAILABLE));
        assert_eq!(bare.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        let conflict = problem_error(&Problem::new(409, code::CONFLICT));
        assert_eq!(conflict.message, "conflict");
        assert_eq!(
            logs.find("nest_rs::mcp", "mcp operation unavailable").len(),
            1,
            "a bare problem or a client one withholds nothing",
        );
    }

    /// rmcp writes the error into the `500` it answers, and its own sentence
    /// names the session a client named: the reply says neither, the chain
    /// the kind alone.
    #[test]
    fn a_session_manager_error_is_said_by_its_kind_never_the_session() {
        let named = SessionsError::from(LocalSessionManagerError::SessionNotFound(
            rmcp::transport::streamable_http_server::session::SessionId::from("named-by-a-client"),
        ));
        assert_eq!(named.to_string(), "the session manager failed");
        assert_eq!(
            nest_rs_core::error_message(&named),
            "the session manager failed: session not found"
        );

        let io = SessionsError::from(LocalSessionManagerError::SessionError(SessionError::Io(
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "sk_live_quoted"),
        )));
        assert_eq!(
            nest_rs_core::error_message(&io),
            "the session manager failed: session I/O failed: broken pipe"
        );
    }

    /// What a domain answers when the ledger refuses; it says no problem.
    #[derive(Debug, thiserror::Error)]
    #[error("account 4111-1111 is frozen")]
    struct Frozen;

    /// What a domain answers when a charge conflicts; it says a `409`.
    #[derive(Debug, thiserror::Error)]
    #[error("charge 42 conflicts with charge 41")]
    struct Conflicting;

    impl ToProblem for Conflicting {
        fn to_problem(&self) -> Option<Problem> {
            Some(Problem::new(409, nest_rs_core::problem::code::CONFLICT))
        }
    }

    #[test]
    fn each_error_is_rendered_by_its_type() {
        use super::ErrorReportChain as _;

        let deliberate = ErrorReport(McpError::invalid_params("bad cursor", None))
            .into_failure()
            .into_answer();
        assert_eq!(deliberate.message, "bad cursor");

        let conflicting = ErrorReport(Conflicting).into_failure();
        assert!(
            conflicting.failure.find::<Conflicting>().is_some(),
            "a filter reads the error as its type"
        );
        let conflicting = conflicting.into_answer();
        assert_eq!(conflicting.code, ErrorCode::INVALID_REQUEST);
        assert_eq!(
            conflicting.data,
            Some(serde_json::json!({ "code": "CONFLICT" }))
        );

        let logs = nest_rs_testing::LogCapture::install();
        let frozen = ErrorReport(Frozen).into_failure();
        assert!(frozen.failure.find::<Frozen>().is_some());
        let frozen = frozen.into_answer();
        assert_eq!(frozen.message, OPAQUE);
        assert!(!format!("{frozen:?}").contains("4111"), "{frozen:?}");
        let event = logs.expect_one(crate::TARGET, "mcp operation failed");
        assert_eq!(
            event.field("error").as_deref(),
            Some("account 4111-1111 is frozen")
        );

        let carried = nest_rs_core::anyhow::Error::from(Problem::new(
            404,
            nest_rs_core::problem::code::NOT_FOUND,
        ))
        .context("loading the post");
        let found = ErrorReport(carried).into_failure().into_answer();
        assert_eq!(found.message, "not found");

        let piped = ErrorReport(PipeError::new("must be a valid UUID"))
            .into_failure()
            .into_answer();
        assert_eq!(piped.message, "must be a valid UUID");
        assert_eq!(piped.code, ErrorCode::INVALID_PARAMS);

        let decode = serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        let said = ErrorReport(decode).into_failure().into_answer();
        assert!(!said.message.contains("sk_live"), "{}", said.message);
        assert_eq!(said.code, ErrorCode::INVALID_PARAMS);
    }

    /// What an operation wrote goes out when no filter answered over it.
    #[test]
    fn an_answered_failure_goes_out_as_the_operation_wrote_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let OperationFailure { failure, reply } =
            OperationFailure::from(McpError::invalid_params("bad cursor", None));
        let failure = failure.answered(nest_rs_core::__private::settled());

        let written = answer(&failure, reply);
        assert_eq!(
            written.message, "bad cursor",
            "no filter: the operation's own"
        );
        logs.expect_none(crate::TARGET, "mcp operation failed");
    }
}

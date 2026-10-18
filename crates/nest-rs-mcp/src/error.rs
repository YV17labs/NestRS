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

use nest_rs_core::__private::Withheld;
use nest_rs_core::ToProblem;
use nest_rs_pipes::PipeError;
use rmcp::ErrorData as McpError;
use rmcp::model::ErrorCode;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManagerError, SessionError,
};

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
/// [`Problem`](nest_rs_core::Problem) among them — as the JSON-RPC error an MCP
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
    let Some(problem) = error.to_problem() else {
        return opaque_error(error);
    };
    match nest_rs_core::__private::withheld(&problem, error) {
        Some(Withheld::Failed(said)) => failed(&said),
        Some(Withheld::Unavailable(said)) => unavailable(&said),
        None => {}
    }
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

/// The opaque answer, with `error`'s whole chain on the operator's line.
fn opaque_error(error: &(dyn std::error::Error + 'static)) -> McpError {
    failed(&nest_rs_core::error_message(error));
    McpError::internal_error(OPAQUE, None)
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

/// The failure a decorated operation reports when it declares guards and finds
/// no app to resolve them from: closed, never a chain of zero guards.
pub fn unresolvable_chain(label: &'static str) -> McpError {
    tracing::error!(
        target: crate::TARGET,
        operation = label,
        reason = "no_ambient_container",
        "mcp operation declares guards but the mount carries no container",
    );
    McpError::internal_error(OPAQUE, None)
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

/// What an MCP operation answers: a `Result` whose error takes an [`McpError`],
/// whatever the type is called.
#[diagnostic::on_unimplemented(
    message = "an MCP operation returns `Result<_, McpError>`, and `{Self}` is not one",
    label = "not a `Result` an `McpError` converts into",
    note = "its guard chain, its access posture and its pipes refuse by returning, and an \
            operation that cannot fail has nowhere to report a denial"
)]
pub trait OperationAnswer {
    /// The answer carrying `error`.
    fn refused(error: McpError) -> Self;
}

impl<T, E: From<McpError>> OperationAnswer for Result<T, E> {
    fn refused(error: McpError) -> Self {
        Err(E::from(error))
    }
}

/// A refusal from the wrapper's chain, gate or pipes, as the operation's own
/// answer — see [`OperationAnswer`].
pub fn refused<R: OperationAnswer>(error: McpError) -> R {
    R::refused(error)
}

/// Why a mount's session manager refused: its cap, or rmcp's own manager.
///
/// rmcp writes the `Display` into the `500` it answers a refused session with,
/// so neither variant names the session.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionsError {
    /// The mount holds its `max_concurrent_sessions`.
    #[error("the mount holds its maximum of live sessions")]
    Full,
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

    #[test]
    fn an_operation_with_guards_and_no_container_says_which_operation_and_why() {
        let logs = nest_rs_testing::LogCapture::install();
        let _ = unresolvable_chain("posts::publish");

        let event = logs.expect_one(
            "nest_rs::mcp",
            "mcp operation declares guards but the mount carries no container",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("operation").as_deref(), Some("posts::publish"));
        assert_eq!(
            event.field("reason").as_deref(),
            Some("no_ambient_container"),
        );
    }
}

//! What a failing operation tells the model, and what it tells the operator.
//!
//! A tool body talks to a **language model**, which may repeat whatever an
//! error's `Display` carries; [`Opaque`] logs the real error and answers an opaque one.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_mcp::{AllowAllMcpGuard, DEFAULT_PATH, McpError, McpOperationGuard, Opaque, mcp, tools};
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
//! # #[module(providers = [RowsService, RowsHost, AllowAllMcpGuard as dyn McpOperationGuard])]
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

use nest_rs_core::ToProblem;
use nest_rs_pipes::PipeError;
use rmcp::ErrorData as McpError;

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
    if let Some(said) = nest_rs_core::__private::withheld(&problem, error) {
        failed(&said);
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
        let failed = logs.expect_one("nest_rs::mcp", "mcp operation failed");
        assert_eq!(failed.level, "error");
        assert!(
            failed
                .field("error")
                .is_some_and(|e| e.contains("10.0.0.1")),
            "{failed:#?}"
        );

        let bare = problem_error(&Problem::new(503, code::UNAVAILABLE));
        assert_eq!(bare.message, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        let conflict = problem_error(&Problem::new(409, code::CONFLICT));
        assert_eq!(conflict.message, "conflict");
        assert_eq!(
            logs.find("nest_rs::mcp", "mcp operation failed").len(),
            1,
            "a bare problem or a client one withholds nothing",
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

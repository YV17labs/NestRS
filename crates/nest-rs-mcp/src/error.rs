//! What a failing operation tells the model, and what it tells the operator.
//!
//! A tool body talks to a **language model**, and a model is not a trusted
//! reader: whatever an error's `Display` carries — a SQL fragment, a column
//! name, a row value, a bucket key — is handed to it verbatim and may be
//! repeated to whoever is chatting. Every host in this repo had therefore grown
//! the same four lines by hand: log the real error, return an opaque one. Three
//! copies of a security posture is three chances to forget it, so it lives here
//! instead.
//!
//! ```ignore
//! let rows = CrudService::list(&*self.svc).await.opaque()?;
//! ```
//!
//! The operator loses nothing: the real error is emitted at `error` level on
//! `nest_rs::mcp`, inside rmcp's own per-operation span, so a log search still
//! reaches it from the failing call.
//!
//! **A deliberate error is not this.** `McpError::invalid_params(…)` exists to
//! be *read* by the model so it can retry with corrected input — return it
//! directly and never route it through here.

use std::fmt::Display;

use nest_rs_pipes::PipeError;
use rmcp::ErrorData as McpError;

/// What the model is told when a failure is none of its business. The value is
/// `nest-rs-core`'s, shared with GraphQL and WS so a client cannot tell from the
/// wording which transport it hit.
use nest_rs_core::OPAQUE_CLIENT_MESSAGE as OPAQUE;

/// Turn a failure the model must not read into one it may.
///
/// Implemented for every `Result` whose error is printable, so it covers a
/// `DbErr`, a storage error, an `anyhow::Error` and a feature's own type without
/// any of them having to know MCP exists.
///
/// The twin traits on GraphQL and WS are deliberately separate types rather than
/// one trait generic over the error: the output is what lets `.opaque()?` infer
/// from the enclosing function's return type. See `nest_rs_core::opaque`.
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the model an opaque one.
    fn opaque(self) -> Result<T, McpError>;
}

impl<T, E: Display> Opaque<T> for Result<T, E> {
    fn opaque(self) -> Result<T, McpError> {
        self.map_err(|err| {
            tracing::error!(
                target: "nest_rs::mcp",
                error = %err,
                "mcp operation failed",
            );
            McpError::internal_error(OPAQUE, None)
        })
    }
}

/// Render a rejected pipe as the one MCP error a model can act on.
///
/// The opposite case to [`Opaque`], and deliberately so: a pipe rejects the
/// model's *own* input, so the message is exactly what lets it retry with
/// corrected arguments. The structured `details` — `ValidationPipe`'s field
/// errors — ride along as the error's data, the same payload the HTTP `400` and
/// the WS error frame carry.
///
/// Emitted by the `#[mcp]` expansion around every `Valid<T>` / `Piped<P, T>`
/// argument; never written by hand.
pub fn pipe_error(err: &PipeError) -> McpError {
    McpError::invalid_params(err.message().to_owned(), err.details().cloned())
}

/// The failure a decorated operation reports when it declares guards and finds
/// no app to resolve them from.
///
/// Only reachable through a mount built without a container — a hand-assembled
/// endpoint, or [`McpMount::deny_all`](crate::McpMount). It fails **closed and
/// named**: an unresolvable chain silently running zero guards is the fail-open
/// reading of the same fact, and it is the reading that ships a tool surface
/// nobody gated.
pub fn unresolvable_chain(label: &'static str) -> McpError {
    tracing::error!(
        target: "nest_rs::mcp",
        operation = label,
        reason = "no_ambient_container",
        "mcp operation declares guards but the mount carries no container",
    );
    McpError::internal_error(OPAQUE, None)
}

#[cfg(test)]
mod tests {
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

    /// The other half of the same contract, and the half nobody read.
    ///
    /// Withholding the cause is only safe because it is recorded somewhere
    /// else — and here the client is a language model, which will happily
    /// repeat whatever it is handed. If this event stopped carrying `error`, a
    /// failing tool would be a blank refusal with no trace, and the
    /// nothing-leaks test next door would still pass: it only asserts what is
    /// *absent* from the reply.
    #[test]
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

    /// A decorated operation that declares guards and finds no container to
    /// resolve them from fails **closed and named**.
    ///
    /// The alternative reading of the same fact — an unresolvable chain
    /// silently running zero guards — ships a tool surface nobody gated, to a
    /// language model. So the refusal is the behaviour and the event is what
    /// makes it diagnosable: the model is handed an opaque error, and
    /// `operation` plus `reason` are the only place the cause exists.
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

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

use rmcp::ErrorData as McpError;

/// What the model is told when an operation fails for a reason that is none of
/// its business. Constant on purpose: a message assembled from the error would
/// leak by construction.
const OPAQUE: &str = "internal error";

/// Turn a failure the model must not read into one it may.
///
/// Implemented for every `Result` whose error is printable, so it covers a
/// `DbErr`, a storage error, an `anyhow::Error` and a feature's own type without
/// any of them having to know MCP exists.
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
}

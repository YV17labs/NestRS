//! MCP data-layer binding (feature `mcp`).
//!
//! rmcp dispatches every tool call on its own spawned task, so the ORM executor
//! and authz ability the HTTP request installed are gone by the time a tool
//! body runs. This implements `nest-rs-mcp`'s [`McpToolContext`] seam to
//! re-install both around each operation — the same
//! [`with_data_context`](crate::dispatch::with_data_context) every other
//! after-the-request transport uses, so the transaction semantics cannot drift
//! between them.
//!
//! The endpoint's operation guard (`McpAbilityBridge`) runs the same
//! authn/authz chain controllers use and attaches the caller's ability to the
//! request; this bridge captures it there — post-guard, still on the HTTP task.
//! It does **not** re-run the guard chain.
//!
//! **The ability install here is not redundant with the guard's `around`, do
//! not "de-duplicate" it.** Both read the same `Arc<Ability>` off the same
//! request, so they cannot disagree — but a guard installs one only if it
//! implements `McpOperationGuard::capture`, which is optional and defaults to
//! `None`. A custom guard that attaches an ability in `before` and leaves
//! `capture` alone would leave a tool body silently unscoped if this stopped
//! installing it. `with_data_context` is the data layer's own fail-safe, and it
//! is the *only* installer on the WS path, which has no `around` seam at all.
//!
//! A tool running with no `Ability` gets `Repo`'s fail-closed behaviour:
//! `scope_for` denies every row.

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_mcp::{BoxFuture, Captured, McpError, McpToolContext, OperationOutcome};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::dispatch::{RequestSnapshot, with_data_context};

/// Re-installs the data context for a tool host's operations. List it
/// `as dyn McpToolContext` on the tool's module.
#[injectable]
pub struct McpDataContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl McpToolContext for McpDataContext {
    fn capture(&self, req: &Request) -> Captured {
        Arc::new(RequestSnapshot::capture(&self.db, req))
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        Box::pin(with_data_context(
            captured,
            "mcp",
            inner,
            |outcome| outcome.is_ok(),
            || Err(McpError::internal_error("internal error".to_string(), None)),
        ))
    }
}

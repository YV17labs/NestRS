//! Installing the ability here is not redundant with the guard's `around`: a
//! guard leaving the optional `McpOperationGuard::capture` unimplemented would
//! otherwise leave a tool body unscoped.

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
            || {
                Err(McpError::internal_error(
                    nest_rs_core::OPAQUE_CLIENT_MESSAGE,
                    None,
                ))
            },
        ))
    }
}

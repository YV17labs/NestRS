//! Default deny-all guard for MCP endpoints mounted without an explicit
//! [`McpOperationGuard`](crate::guard::McpOperationGuard).

use poem::http::StatusCode;
use poem::{Error, Request, Response};

use crate::guard::McpOperationGuard;

pub(crate) struct DenyAllMcpGuard;

impl McpOperationGuard for DenyAllMcpGuard {
    fn before<'a>(&'a self, _req: &'a mut Request) -> crate::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move {
            Err(Error::from_response(
                Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body("unauthorized"),
            ))
        })
    }
}

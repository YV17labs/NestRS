use std::sync::Arc;

use nest_rs::mcp::rmcp;
use nest_rs::mcp::{
    CallToolResult, ContentBlock, McpError, ServerHandler, mcp, tool, tool_handler, tool_router,
};
use nest_rs::seaorm::CrudService;

use crate::users::UsersService;

#[mcp(path = "/mcp")]
#[derive(Clone)]
pub struct UsersTool {
    #[inject]
    svc: Arc<UsersService>,
}

#[tool_router]
impl UsersTool {
    #[tool(
        description = "List the people the caller is allowed to see, by name. \
                       Scoped to the caller's organization."
    )]
    async fn list_people(&self) -> Result<CallToolResult, McpError> {
        let rows = CrudService::list(&*self.svc).await.map_err(|err| {
            tracing::error!(target: "features::users", error = %err, "mcp people lookup failed");
            McpError::internal_error("internal error".to_owned(), None)
        })?;

        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        Ok(CallToolResult::success(vec![ContentBlock::text(
            if names.is_empty() {
                "no readable people".to_owned()
            } else {
                names.join("\n")
            },
        )]))
    }
}

#[tool_handler]
impl ServerHandler for UsersTool {}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs::core::Discoverable;

    use super::UsersTool;
    use crate::users::UsersService;

    #[test]
    fn mcp_tool_declares_its_injected_service_for_the_access_graph() {
        assert!(
            UsersTool::injected().contains(&TypeId::of::<UsersService>()),
            "the MCP tool's injected UsersService is recorded for the access graph",
        );
    }
}

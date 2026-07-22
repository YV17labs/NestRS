use std::sync::Arc;

use nest_rs_mcp::{
    CallToolResult, ContentBlock, McpError, ServerHandler, mcp, tool, tool_handler, tool_router,
};
use nest_rs_seaorm::CrudService;

use crate::posts::PostsService;

#[mcp(path = "/posts/mcp")]
#[derive(Clone)]
pub struct PostsTool {
    #[inject]
    svc: Arc<PostsService>,
}

#[tool_router]
impl PostsTool {
    #[tool(
        description = "List the post titles the caller is allowed to read, most \
                       recent first. Scoped to the caller's organization."
    )]
    async fn list_posts(&self) -> Result<CallToolResult, McpError> {
        let rows = CrudService::list(&*self.svc)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        Ok(CallToolResult::success(vec![ContentBlock::text(
            if titles.is_empty() {
                "no readable posts".to_owned()
            } else {
                titles.join("\n")
            },
        )]))
    }
}

#[tool_handler]
impl ServerHandler for PostsTool {}

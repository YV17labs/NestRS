use std::sync::Arc;

use nest_rs::authz::Action;
use nest_rs::mcp::model::{
    GetPromptResult, ListResourcesResult, PaginatedRequestParams, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerInfo,
};
use nest_rs::mcp::rmcp;
use nest_rs::mcp::service::{RequestContext, RoleServer};
use nest_rs::mcp::{
    CallToolResult, ContentBlock, McpError, Opaque, ServerHandler, mcp, prompt, prompt_handler,
    prompt_router, tool, tool_handler, tool_router,
};
use nest_rs::seaorm::{Access, CrudService};
use uuid::Uuid;

use crate::posts::PostsService;

const POST_URI_PREFIX: &str = "post://";

#[mcp(
    path = "/mcp/posts",
    name = "nestrs-assistant-posts",
    title = "nestrs demo assistant — posts"
)]
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
        let rows = CrudService::list(&*self.svc).await.opaque()?;

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

#[prompt_router]
impl PostsTool {
    #[prompt(
        description = "Draft a follow-up post, primed with the titles the caller \
                       can already read."
    )]
    async fn draft_follow_up(&self) -> Result<GetPromptResult, McpError> {
        let rows = CrudService::list(&*self.svc).await.opaque()?;
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Draft a follow-up post. Existing titles:\n{}",
                if titles.is_empty() {
                    "(none readable)".to_owned()
                } else {
                    titles.join("\n")
                }
            ),
        )])
        .with_description("Follow-up draft primed with the caller's readable posts"))
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for PostsTool {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let rows = CrudService::list(&*self.svc).await.opaque()?;

        Ok(ListResourcesResult {
            resources: rows
                .iter()
                .map(|row| Resource::new(format!("{POST_URI_PREFIX}{}", row.id), row.title.clone()))
                .collect(),
            ..ListResourcesResult::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let id = request
            .uri
            .strip_prefix(POST_URI_PREFIX)
            .and_then(|raw| Uuid::parse_str(raw).ok())
            .ok_or_else(|| {
                McpError::resource_not_found(format!("unknown resource `{}`", request.uri), None)
            })?;

        let post = match CrudService::access(&*self.svc, Action::Read, id)
            .await
            .opaque()?
        {
            Access::Found(post) => post,
            Access::Denied | Access::Missing => {
                return Err(McpError::resource_not_found(
                    format!("unknown resource `{}`", request.uri),
                    None,
                ));
            }
        };

        Ok(ReadResourceResult::new(vec![ResourceContents::text(post.body, request.uri)]).into())
    }
}

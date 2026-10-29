use std::sync::Arc;

use nest_rs::authz::{Action, Read};
use nest_rs::mcp::model::{GetPromptResult, PromptMessage, Resource, Role};
use nest_rs::mcp::{Json, McpError, Opaque, mcp, tools};
use nest_rs::seaorm::{Access, CrudService};
use uuid::Uuid;

use crate::posts::{Entity as PostEntity, Post, PostsService};

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

#[tools]
impl PostsTool {
    #[tool(
        description = "List the post titles the caller is allowed to read, most \
                       recent first. Scoped to the caller's organization."
    )]
    #[authorize(Read, PostEntity)]
    async fn list_posts(&self) -> Result<String, McpError> {
        let rows = CrudService::list(&*self.svc).await.opaque()?;

        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        Ok(if titles.is_empty() {
            "no readable posts".to_owned()
        } else {
            titles.join("\n")
        })
    }

    #[prompt(
        description = "Draft a follow-up post, primed with the titles the caller \
                       can already read."
    )]
    #[authorize(Read, PostEntity, unmasked)]
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

    #[resources]
    #[authorize(Read, PostEntity, unmasked)]
    async fn posts(&self) -> Result<Vec<Resource>, McpError> {
        let rows = CrudService::list(&*self.svc).await.opaque()?;

        Ok(rows
            .into_iter()
            .map(|row| Resource::new(format!("post://{}", row.id), row.title))
            .collect())
    }

    #[resource(
        template = "post://{id}",
        description = "One post the caller is allowed to read, as JSON."
    )]
    #[authorize(Read, PostEntity)]
    async fn post(&self, id: Uuid) -> Result<Option<Json<Post>>, McpError> {
        let access = CrudService::access(&*self.svc, Action::Read, id)
            .await
            .opaque()?;
        Ok(match access {
            Access::Found(post) => Some(Json(Post::from(&post))),
            Access::Denied | Access::Missing => None,
        })
    }
}

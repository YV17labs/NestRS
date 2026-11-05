use std::sync::Arc;

use nest_rs::authz::Read;
use nest_rs::mcp::model::{GetPromptResult, PromptMessage, Resource, Role};
use nest_rs::mcp::{Json, mcp, tools};
use nest_rs::seaorm::{Authorized, CrudService, ServiceError};

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
    async fn list_posts(&self) -> Result<String, ServiceError> {
        let rows = self.svc.list().await?;

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
    async fn draft_follow_up(&self) -> Result<GetPromptResult, ServiceError> {
        let rows = self.svc.list().await?;
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
    async fn posts(&self) -> Result<Vec<Resource>, ServiceError> {
        let rows = self.svc.list().await?;

        Ok(rows
            .into_iter()
            .map(|row| Resource::new(format!("post://{}", row.id), row.title))
            .collect())
    }

    #[resource(
        template = "post://{id}",
        description = "One post the caller is allowed to read, as JSON."
    )]
    #[authorize(Read, bind = PostsService)]
    async fn post(&self, post: Authorized<Read, PostEntity>) -> Result<Json<Post>, ServiceError> {
        Ok(Json(Post::from(&post.into_inner())))
    }
}

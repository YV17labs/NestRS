use nest_rs_core::{Layer, injectable};
use nest_rs_exception_filters::ExceptionFilter;
use nest_rs_http::async_trait;
use poem::Response;
use poem::http::StatusCode;

use crate::posts::PostError;

#[injectable]
#[derive(Default)]
pub struct PostProblemFilter;

impl Layer for PostProblemFilter {}

#[async_trait]
impl ExceptionFilter for PostProblemFilter {
    type Exception = PostError;

    async fn catch(&self, err: PostError) -> Response {
        let (status, title, kind) = match &err {
            PostError::AlreadyPublished { .. } => (
                StatusCode::CONFLICT,
                "Post already published",
                "https://nestrs.dev/problems/post-already-published",
            ),
        };
        let body = serde_json::json!({
            "type": kind,
            "title": title,
            "status": status.as_u16(),
            "detail": err.to_string(),
        });
        Response::builder()
            .status(status)
            .content_type("application/problem+json")
            .body(serde_json::to_vec(&body).unwrap_or_default())
    }
}

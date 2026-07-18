use nest_rs_core::{Layer, injectable};
use nest_rs_exception_filters::ExceptionFilter;
use nest_rs_http::async_trait;
use poem::Response;
use poem::http::StatusCode;

use crate::posts::PostError;

/// Renders a posts-domain [`PostError`] as RFC 9457
/// `application/problem+json`.
///
/// Bound with `#[use_exception_filters(PostProblemFilter)]` beside the publish
/// handler, it is the typed catch sitting closest to that handler: a
/// `PostError` thrown as a `poem::Error` is downcast here and turned into a
/// problem document, while any other error type falls through untouched to the
/// outer chain. Exception filters, unlike reusable pipes, are legitimately
/// app-defined — this is the product's wire contract for a domain failure.
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
        // RFC 9457 members: a `type` URI identifying the problem class, a human
        // `title`, the numeric `status`, and a request-specific `detail`.
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

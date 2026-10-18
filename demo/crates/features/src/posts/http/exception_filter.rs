use nest_rs::core::{Caught, ExceptionFilter, Layer, UnitContext, async_trait, injectable};
use nest_rs::http::{HttpView, IntoResponse, ProblemDetails};

use crate::posts::PostError;

#[injectable]
#[derive(Default)]
pub struct PostProblemFilter;

impl Layer for PostProblemFilter {}

#[async_trait]
impl ExceptionFilter for PostProblemFilter {
    type Exception = PostError;

    async fn catch(&self, err: &PostError, cx: &mut UnitContext<'_>) -> Caught {
        let PostError::AlreadyPublished { .. } = err else {
            return Caught::Declined;
        };
        let problem = ProblemDetails::conflict()
            .with_type("https://nestrs.dev/problems/post-already-published")
            .with_title("Post already published")
            .with_detail(err.to_string());
        match HttpView::of(cx) {
            Ok(view) => Caught::Answered(view.respond(problem.into_response())),
            Err(_) => Caught::Declined,
        }
    }
}

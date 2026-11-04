//! HTTP's reading of [`Scoped<T>`]: an `#[injectable(scope = request)]`
//! provider, from the scope the transport edge installs per request.

use std::any::type_name;

use nest_rs_core::Scoped;

use crate::problem::ProblemDetails;

/// `T` from the running request's scope, or a miss, the type filed at `error`.
fn resolve<T: Send + Sync + 'static>() -> Option<Scoped<T>> {
    let Some(scope) = nest_rs_core::current_request_scope() else {
        tracing::error!(
            target: crate::target::HTTP,
            provider = type_name::<T>(),
            "request scope not installed — the transport edge must wrap the route tree",
        );
        return None;
    };
    let Some(provider) = scope.get::<T>() else {
        tracing::error!(
            target: crate::target::HTTP,
            provider = type_name::<T>(),
            "no provider for a request-scoped parameter — add it to a module's providers",
        );
        return None;
    };
    Some(Scoped(provider))
}

/// Reads `T` from the scope the transport edge installed. Should it still be
/// missing at a request, the request answers an opaque `500` and the type is
/// filed at `error`, never answered.
impl<'a, T: Send + Sync + 'static> crate::FromRequest<'a> for Scoped<T> {
    async fn from_request(
        _req: &'a crate::Request,
        _body: &mut crate::RequestBody,
    ) -> crate::Result<Self> {
        resolve().ok_or_else(|| ProblemDetails::internal().into())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::{Container, Correlation, RequestScope};
    use nest_rs_testing::LogCapture;

    use super::*;
    use crate::{FromRequest, Request, StatusCode};

    struct Marker(&'static str);

    async fn in_scope<F: std::future::Future>(container: Container, extract: F) -> F::Output {
        let scope = Arc::new(RequestScope::new(container));
        nest_rs_core::with_request_scope(Some(scope), Correlation::minted(None), extract).await
    }

    #[tokio::test]
    async fn scoped_from_request_resolves_a_registered_provider() {
        // A singleton falls through `RequestScope::get`.
        let container = Container::builder()
            .provide(Marker("registered"))
            .build()
            .expect("the container builds");
        let (req, mut body) = Request::builder().finish().split();
        let scoped = in_scope(container, Scoped::<Marker>::from_request(&req, &mut body))
            .await
            .expect("resolves via singleton fallback");
        assert_eq!(scoped.0.0, "registered");
    }

    /// The body a miss answers, asserted to be the one opaque `500`: its
    /// class's code, no detail.
    async fn opaque_body(error: crate::HttpError) -> String {
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let text = crate::testing::text(response).await;
        let document: serde_json::Value = serde_json::from_str(&text).expect("problem json");
        assert_eq!(document, crate::testing::internal_document());
        text
    }

    #[tokio::test]
    async fn a_miss_answers_an_opaque_500_and_files_the_type_at_error() {
        let logs = LogCapture::install();
        let (req, mut body) = Request::builder().finish().split();
        let Err(refused) = in_scope(
            Container::builder().build().expect("the container builds"),
            Scoped::<Marker>::from_request(&req, &mut body),
        )
        .await
        else {
            panic!("no provider for Marker");
        };
        let text = opaque_body(refused).await;
        assert!(
            !text.contains("Marker"),
            "the type stays off the wire: {text}"
        );

        let event = logs.expect_one(
            "nest_rs::http",
            "no provider for a request-scoped parameter — add it to a module's providers",
        );
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("provider")
                .is_some_and(|provider| provider.contains("Marker")),
            "{:?}",
            event.fields,
        );
    }

    #[tokio::test]
    async fn a_request_with_no_scope_answers_an_opaque_500_and_files_it_at_error() {
        let logs = LogCapture::install();
        let (req, mut body) = Request::builder().finish().split();
        let Err(refused) = Scoped::<Marker>::from_request(&req, &mut body).await else {
            panic!("no ambient request scope");
        };
        let text = opaque_body(refused).await;
        assert!(!text.contains("Marker"), "{text}");
        assert_eq!(
            logs.expect_one(
                "nest_rs::http",
                "request scope not installed — the transport edge must wrap the route tree",
            )
            .level,
            "error"
        );
    }
}

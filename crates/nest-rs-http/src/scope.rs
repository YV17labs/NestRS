//! HTTP binding for request-scoped providers: [`Scoped<T>`] resolves an
//! `#[injectable(scope = request)]` provider from the scope the transport edge
//! installs per request.

use std::any::{TypeId, type_name};
use std::ops::Deref;
use std::sync::Arc;

use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, ResolvesProvider};

use crate::problem::ProblemDetails;

/// Resolves a provider of type `T` from the current request's
/// [`RequestScope`](nest_rs_core::RequestScope) the transport edge installed.
///
/// The provider is part of the boot's access contract
/// ([`ResolvesProvider`]). Should it still be missing at a request — the
/// scope not installed, or `T` offered by no module — the request answers an
/// opaque `500` and the type is filed at `error`, never answered.
///
/// ```
/// use std::any::TypeId;
///
/// use nest_rs_core::ResolvesProvider;
/// use nest_rs_http::Scoped;
///
/// struct Cart;
///
/// assert_eq!(Scoped::<Cart>::provider().0, TypeId::of::<Cart>());
/// ```
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    /// Take the resolved provider handle out of the extractor.
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Scoped<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: 'static> ResolvesProvider for Scoped<T> {
    fn provider() -> (TypeId, &'static str) {
        (TypeId::of::<T>(), type_name::<T>())
    }
}

impl<T: Send + Sync + 'static> Scoped<T> {
    /// `T` from the running request's scope, or a miss, the type filed at
    /// `error`.
    fn resolve() -> Option<Self> {
        let Some(scope) = crate::current_request_scope() else {
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
}

/// What a miss answers: the client learns nothing of the wiring.
fn opaque() -> ProblemDetails {
    ProblemDetails::internal().with_detail(OPAQUE_CLIENT_MESSAGE)
}

impl<'a, T: Send + Sync + 'static> crate::FromRequest<'a> for Scoped<T> {
    async fn from_request(
        _req: &'a crate::Request,
        _body: &mut crate::RequestBody,
    ) -> crate::Result<Self> {
        Self::resolve().ok_or_else(|| opaque().into())
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::{ParamProbe, ParamProbeFallback as _, ParamProbeResolves as _};
    use nest_rs_core::{Container, Correlation, RequestScope};
    use nest_rs_testing::LogCapture;

    use super::*;
    use crate::{FromRequest, Request, StatusCode};

    struct Marker(&'static str);

    #[test]
    fn scoped_into_inner_yields_the_arc() {
        let value: Arc<Marker> = Arc::new(Marker("hi"));
        let scoped = Scoped(value.clone());
        let inner = scoped.into_inner();
        assert!(Arc::ptr_eq(&inner, &value));
    }

    #[test]
    fn scoped_deref_borrows_the_inner_value() {
        let scoped = Scoped(Arc::new(Marker("bye")));
        assert_eq!(scoped.0.as_ref().0, "bye");
        assert_eq!((*scoped).0, "bye");
    }

    /// The probe `#[routes]` asks per parameter, written once at the
    /// reference depth its expansion calls it.
    macro_rules! probe {
        ($ty:ty) => {
            (&&ParamProbe::<$ty>::new()).resolved_provider()
        };
    }

    #[test]
    fn the_handler_probe_reads_the_provider_scoped_resolves() {
        let resolved = probe!(Scoped<Marker>);
        assert_eq!(resolved.map(|(id, _)| id), Some(TypeId::of::<Marker>()));
        assert!(resolved.is_some_and(|(_, label)| label.ends_with("Marker")));
        assert_eq!(probe!(Arc<Marker>), None);
    }

    async fn in_scope<F: std::future::Future>(container: Container, extract: F) -> F::Output {
        let scope = Arc::new(RequestScope::new(container));
        crate::with_request_scope(Some(scope), Correlation::minted(None), extract).await
    }

    #[tokio::test]
    async fn scoped_from_request_resolves_a_registered_provider() {
        // A singleton falls through `RequestScope::get`.
        let container = Container::builder().provide(Marker("registered")).build();
        let (req, mut body) = Request::builder().finish().split();
        let scoped = in_scope(container, Scoped::<Marker>::from_request(&req, &mut body))
            .await
            .expect("resolves via singleton fallback");
        assert_eq!(scoped.0.0, "registered");
    }

    async fn opaque_body(error: crate::HttpError) -> String {
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        response.into_body().into_string().await.unwrap()
    }

    #[tokio::test]
    async fn a_miss_answers_an_opaque_500_and_files_the_type_at_error() {
        let logs = LogCapture::install();
        let (req, mut body) = Request::builder().finish().split();
        let Err(refused) = in_scope(
            Container::builder().build(),
            Scoped::<Marker>::from_request(&req, &mut body),
        )
        .await
        else {
            panic!("no provider for Marker");
        };
        let text = opaque_body(refused).await;
        assert!(text.contains(OPAQUE_CLIENT_MESSAGE), "{text}");
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

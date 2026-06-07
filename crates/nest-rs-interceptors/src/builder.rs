//! Adds [`AppBuilderInterceptorsExt::use_interceptors_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use std::sync::Arc;

use nest_rs_core::AppBuilder;
use nest_rs_http::{HttpInterceptorMeta, http_interceptor_priority};
use poem::endpoint::BoxEndpoint;
use poem::{EndpointExt, Response};

use crate::ext::InterceptorExt;
use crate::registry::{InterceptorSpec, InterceptorSpecs};

/// Adds `.use_interceptors_global(...)` to [`AppBuilder`].
///
/// ```rust,ignore
/// use nest_rs_interceptors::{AppBuilderInterceptorsExt, interceptor};
///
/// App::builder()
///     .use_interceptors_global([interceptor::<ServerTiming>()])
///     .module::<AppModule>()
///     .build().await?
///     .run().await
/// ```
///
/// Declaration order matters: the runtime chain wraps the handler in the
/// reverse order of declaration (first listed = outermost), with
/// [`Layer::priority`](nest_rs_core::Layer::priority) as an optional
/// tiebreaker.
///
/// Behind the scenes this seeds two things at once:
///
/// 1. [`InterceptorSpecs`] into the container — the per-route shaper
///    ([`LayersRouteInterceptor`](../../nest_rs_guards/integration/struct.LayersRouteInterceptor.html))
///    reads them for TypeId-based dedup against controller / method
///    declarations.
/// 2. An [`HttpInterceptorMeta`] wrap that resolves the specs at HTTP
///    `configure` time and folds every global interceptor around the
///    assembled endpoint — so they fire on every endpoint the HTTP
///    transport mounts, including self-mounting routes like `/graphql`,
///    MCP, and WS upgrade.
pub trait AppBuilderInterceptorsExt: Sized {
    fn use_interceptors_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = InterceptorSpec>;
}

impl AppBuilderInterceptorsExt for AppBuilder {
    fn use_interceptors_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = InterceptorSpec>,
    {
        let collected: Vec<InterceptorSpec> = specs.into_iter().collect();
        self.provide(InterceptorSpecs(collected))
            .provide_meta(HttpInterceptorMeta::with_priority(
                http_interceptor_priority::INTERCEPTORS,
                |container, mut endpoint: BoxEndpoint<'static, Response>| {
                    let Some(specs) = container.get::<InterceptorSpecs>() else {
                        return endpoint;
                    };
                    // Reverse so the first-listed entry ends up outermost
                    // (matches per-route ordering — declaration order is the
                    // call order).
                    for spec in specs.0.iter().rev() {
                        if let Some(interceptor) = spec.resolve(container) {
                            let arc: Arc<dyn crate::Interceptor> = interceptor;
                            endpoint = InterceptorExt::interceptor(endpoint, arc)
                                .map_to_response()
                                .boxed();
                        } else {
                            tracing::warn!(
                                target: "nest_rs::layers",
                                layer = spec.name,
                                "global interceptor not registered — skipping at runtime (boot-time access-graph validation should have caught this)",
                            );
                        }
                    }
                    endpoint
                },
            ))
    }
}

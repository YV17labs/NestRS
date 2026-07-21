//! Adds [`AppBuilderInterceptorsExt::use_interceptors_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain};
use nest_rs_core::{AppBuilder, Container, check_specs_resolvable};
use nest_rs_http::{HttpBootCheck, HttpEndpointWrap, endpoint_wrap_priority};
use poem::EndpointExt;

use crate::ext::InterceptorExt;
use crate::interceptor::Interceptor;
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
/// Declaration order matters: the chain wraps in reverse order of
/// declaration (first listed = outermost), with
/// [`Layer::priority`](nest_rs_core::Layer::priority) as an optional
/// tiebreaker.
///
/// This seeds [`InterceptorSpecs`] into the container and attaches the
/// transport-edge wrap that executes the **global** sub-chain around the
/// whole routing tree (band
/// [`POOL_INTERCEPTORS`](nest_rs_http::endpoint_wrap_priority::POOL_INTERCEPTORS)):
/// a global interceptor observes every response leaving the app — guard
/// denials, 404s, self-mounted surfaces (`/graphql`, WS upgrades) included.
/// It therefore runs *before* authentication: no principal, ability or
/// ambient executor is available to it. For actor-aware work, declare the
/// interceptor at the controller / method scope instead — those execute
/// inside the route's guard chain. The per-route composer dedups
/// controller / method redeclarations against this global scope by `TypeId`
/// (broadest wins), so any interceptor still executes exactly once.
pub trait AppBuilderInterceptorsExt: Sized {
    /// Register `specs` as the global interceptor chain — the transport-edge
    /// pool that runs before authentication, deduped by type against
    /// controller/method scope (broadest wins).
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
            .provide_meta(HttpBootCheck::new(|container| {
                let Some(specs) = container.get::<InterceptorSpecs>() else {
                    return Ok(());
                };
                check_specs_resolvable(
                    &specs.0,
                    container,
                    "interceptor",
                    "an unresolvable global interceptor would silently drop",
                )
            }))
            .provide_meta(HttpEndpointWrap::with_priority(
                endpoint_wrap_priority::POOL_INTERCEPTORS,
                |container, endpoint| {
                    let chain = global_chain(container);
                    // `compose_chain` orders outermost-first; wrapping
                    // applies the last entry innermost, so reverse to keep
                    // the first entry outermost.
                    let mut ep = endpoint;
                    for entry in chain.into_iter().rev() {
                        ep = InterceptorExt::interceptor(ep, entry.layer)
                            .map_to_response()
                            .boxed();
                    }
                    ep
                },
            ))
    }
}

/// Resolve `InterceptorSpecs` into the deduplicated, priority-ordered global
/// chain. Composed through the same `compose_chain` as every other Layer
/// System site — this is where an intra-global duplicate is warned about,
/// once.
fn global_chain(container: &Container) -> Vec<ResolvedLayer<dyn Interceptor>> {
    let mut global: Vec<ResolvedLayer<dyn Interceptor>> = Vec::new();
    if let Some(specs) = container.get::<InterceptorSpecs>() {
        for spec in &specs.0 {
            if let Some(layer) = spec.resolve(container) {
                global.push(ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source: LayerSite::Global,
                    layer,
                });
            }
        }
    }
    compose_chain::<dyn Interceptor>(global, Vec::new(), Vec::new(), &[], "transport")
}

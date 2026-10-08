//! Adds [`AppBuilderInterceptorsExt::use_interceptors_global`] to
//! [`AppBuilder`].

use nest_rs_core::layer_chain::{ResolvedLayer, compose_chain, resolve_global_layers};
use nest_rs_core::{AppBuilder, Container, check_specs_resolvable};
use nest_rs_http::{HttpBootCheck, HttpEndpointWrap, endpoint_wrap_priority};
use poem::EndpointExt;

use crate::interceptor::{Interceptor, InterceptorChain};
use crate::registry::{InterceptorSpec, InterceptorSpecs};

/// Adds `.use_interceptors_global(...)` to [`AppBuilder`].
///
/// The example on [`interceptor`](fn@crate::interceptor) registers through it.
/// The first listed is outermost, [`Layer::priority`](nest_rs_core::Layer::priority)
/// breaking ties. The chain wraps the whole routing tree at the transport edge (band
/// [`POOL_INTERCEPTORS`](nest_rs_http::endpoint_wrap_priority::POOL_INTERCEPTORS)),
/// so it sees denials, 404s and self-mounts, and runs *before* authentication: for
/// actor-aware work, declare the interceptor at controller or method scope.
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
                    if chain.is_empty() {
                        return endpoint;
                    }
                    InterceptorChain::new(endpoint, chain).boxed()
                },
            ))
    }
}

/// Resolve `InterceptorSpecs` into the deduplicated, priority-ordered global
/// chain; an intra-global duplicate is warned about here, once.
fn global_chain(container: &Container) -> Vec<ResolvedLayer<dyn Interceptor>> {
    let global = resolve_global_layers::<InterceptorSpecs>(container);
    compose_chain::<dyn Interceptor>(global, Vec::new(), Vec::new(), &[], "transport")
}

//! Adds [`AppBuilderInterceptorsExt::use_interceptors_global`] to
//! [`AppBuilder`].

use nest_rs_core::__private::{
    ResolvedLayer, check_specs_resolvable, compose_chain, resolve_global_layers,
};
use nest_rs_core::{AppBuilder, Container};
use nest_rs_http::__private::{HttpEndpointWrap, endpoint_wrap_priority};
use poem::EndpointExt;

use crate::interceptor::{Interceptor, InterceptorChain};
use crate::registry::{InterceptorSpec, InterceptorSpecs};

/// Adds `.use_interceptors_global(...)` to [`AppBuilder`].
///
/// The example on [`interceptor`](fn@crate::interceptor) registers through it.
/// The first listed is outermost, [`Layer::priority`](nest_rs_core::Layer::priority)
/// breaking ties. The chain wraps the whole routing tree at the transport edge
/// (the global interceptors' band), so it sees denials, 404s and self-mounts,
/// and runs *before* authentication: for actor-aware work, declare the
/// interceptor at controller or method scope.
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
            .provide_wiring(GLOBAL_POOL, check_global_pool)
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

/// The global interceptor pool, as a boot failure names its check.
const GLOBAL_POOL: &str = "nest_rs::interceptors::global";

/// Refuse a global interceptor no imported module provides, in every app — one
/// that serves no HTTP included — before any hook runs.
fn check_global_pool(container: &Container) -> nest_rs_core::anyhow::Result<()> {
    let Some(specs) = container.get::<InterceptorSpecs>() else {
        return Ok(());
    };
    check_specs_resolvable(
        &specs.0,
        container,
        "interceptor",
        "an unresolvable global interceptor would silently drop",
    )
    .map_err(nest_rs_core::anyhow::Error::msg)
}

/// Resolve `InterceptorSpecs` into the deduplicated, priority-ordered global
/// chain; an intra-global duplicate is warned about here, once.
fn global_chain(container: &Container) -> Vec<ResolvedLayer<dyn Interceptor>> {
    let global = resolve_global_layers::<InterceptorSpecs>(container);
    compose_chain::<dyn Interceptor>(global, Vec::new(), Vec::new(), &[], "transport")
}

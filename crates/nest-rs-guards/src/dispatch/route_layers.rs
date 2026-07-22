//! Mount-time composition of the response-side layer pools for one HTTP
//! route: exception-filters, filters, interceptors.
//!
//! Guards and pipes run *inside* [`RouteShaper`] at request time (they are
//! request-side: gate, then transform the body). The response-side families
//! wrap the endpoint itself — they need to see the response / error on the
//! way out — so the `#[routes]` macro composes them here at mount time, all
//! through the **same** `compose_chain` dedup as every other layer kind.
//!
//! Execution sites differ by scope for interceptors and filters:
//!
//! - **Global** interceptors / filters execute at the **transport edge**
//!   (`use_interceptors_global` / `use_filters_global` attach an
//!   `HttpEndpointWrap`) so they also cover 404s, self-mounted surfaces and
//!   guard denials. Here they participate in the dedup only — a controller /
//!   method redeclaration of a global layer is dropped (broadest wins) and
//!   the layer still runs exactly once, at the edge.
//! - **Controller / method** interceptors / filters wrap the handler here,
//!   inside the route's guard chain — a denial short-circuits before them.
//!
//! Exception-filters are handler-scoped by nature (a typed `try_catch`
//! around the handler), so **all three scopes** execute here, closest to the
//! handler — before generic filters get a chance to map the error away.
//!
//! [`RouteShaper`]: crate::dispatch::RouteShaper

use nest_rs_core::layer_chain::{
    LayerSite, ResolvedLayer, compose_chain, dedup_bucket, resolve_global_layers,
};
use nest_rs_core::{Container, MappedError};
use nest_rs_exception_filters::{ExceptionFilterErased, ExceptionFilterSpecs};
use nest_rs_filters::{Filter, FilterEndpoint, FilterSpecs};
use nest_rs_interceptors::{Interceptor, InterceptorExt, InterceptorSpecs};
use poem::endpoint::BoxEndpoint;
use poem::{Endpoint, EndpointExt, Request, Response};

use crate::dispatch::scoped_spec::{
    ScopedExceptionFilterSpec, ScopedFilterSpec, ScopedInterceptorSpec, resolve_specs,
};

/// Wrap `endpoint` in the route-scoped part of the interceptor pool. The full
/// chain (global + controller + method) is composed for dedup; only the
/// controller / method survivors wrap here — global interceptors execute at
/// the transport edge. First-listed ends up outermost. Called by the
/// `#[routes]` macro at mount time.
pub fn wrap_route_interceptors(
    container: &Container,
    endpoint: BoxEndpoint<'static, Response>,
    controller: &[ScopedInterceptorSpec],
    method: &[ScopedInterceptorSpec],
    route_label: &str,
) -> BoxEndpoint<'static, Response> {
    let global = resolve_global_interceptors(container);
    let controller = resolve_specs(container, controller, LayerSite::Controller);
    let method = resolve_specs(container, method, LayerSite::Method);
    let chain = compose_chain::<dyn Interceptor>(global, controller, method, &[], route_label);
    // `compose_chain` orders the list outermost-first; wrapping applies the
    // last entry innermost, so iterate in reverse to keep the first entry
    // outermost.
    let mut ep = endpoint;
    for entry in chain
        .into_iter()
        .filter(|e| e.source != LayerSite::Global)
        .rev()
    {
        ep = InterceptorExt::interceptor(ep, entry.layer)
            .map_to_response()
            .boxed();
    }
    ep
}

/// Wrap `endpoint` in the route-scoped part of the filter pool (error path).
/// Same site rule as interceptors: the full chain composes for dedup, global
/// filters execute at the transport edge, controller / method survivors wrap
/// here. First-listed ends up outermost on the error path.
pub fn wrap_route_filters(
    container: &Container,
    endpoint: BoxEndpoint<'static, Response>,
    controller: &[ScopedFilterSpec],
    method: &[ScopedFilterSpec],
    route_label: &str,
) -> BoxEndpoint<'static, Response> {
    let global = resolve_global_filters(container);
    let controller = resolve_specs(container, controller, LayerSite::Controller);
    let method = resolve_specs(container, method, LayerSite::Method);
    let chain = compose_chain::<dyn Filter>(global, controller, method, &[], route_label);
    let mut ep = endpoint;
    for entry in chain
        .into_iter()
        .filter(|e| e.source != LayerSite::Global)
        .rev()
    {
        ep = FilterEndpoint::new(ep, entry.layer).boxed();
    }
    ep
}

/// Wrap `endpoint` in the **full** exception-filter pool (global +
/// controller + method, deduped). Exception-filters are typed `try_catch`es
/// around the handler — every scope executes here, closest to the handler,
/// so a typed catch gets the error before a generic `Filter` maps it away.
pub fn wrap_route_exception_filters(
    container: &Container,
    endpoint: BoxEndpoint<'static, Response>,
    controller: &[ScopedExceptionFilterSpec],
    method: &[ScopedExceptionFilterSpec],
    route_label: &str,
) -> BoxEndpoint<'static, Response> {
    let global = resolve_global_layers::<ExceptionFilterSpecs>(container);
    let controller = resolve_specs(container, controller, LayerSite::Controller);
    let method = resolve_specs(container, method, LayerSite::Method);
    let chain = compose_chain::<dyn ExceptionFilterErased>(
        dedup_bucket(global),
        controller,
        method,
        &[],
        route_label,
    );
    if chain.is_empty() {
        return endpoint;
    }
    ExceptionFiltersEndpoint {
        inner: endpoint,
        chain,
    }
    .boxed()
}

/// Resolve the global interceptor bucket for the route-site dedup.
/// Intra-bucket duplicates are dropped silently — the transport edge (the
/// site that executes the global sub-chain) already warned once.
fn resolve_global_interceptors(container: &Container) -> Vec<ResolvedLayer<dyn Interceptor>> {
    dedup_bucket(resolve_global_layers::<InterceptorSpecs>(container))
}

/// Resolve the global filter bucket — see [`resolve_global_interceptors`].
fn resolve_global_filters(container: &Container) -> Vec<ResolvedLayer<dyn Filter>> {
    dedup_bucket(resolve_global_layers::<FilterSpecs>(container))
}

/// Runs the deduped exception-filter chain on the error path: first matching
/// filter wins, the rest of the chain is skipped. A mapped response is
/// tagged [`MappedError`] so the ambient transaction rolls back — the
/// handler failed; a typed catch shapes the client answer, it does not bless
/// the handler's writes.
struct ExceptionFiltersEndpoint {
    inner: BoxEndpoint<'static, Response>,
    chain: Vec<ResolvedLayer<dyn ExceptionFilterErased>>,
}

impl Endpoint for ExceptionFiltersEndpoint {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        match self.inner.call(req).await {
            Ok(resp) => Ok(resp),
            Err(err) => {
                let mut current = err;
                for entry in &self.chain {
                    match entry.layer.try_catch(current).await {
                        Ok(mut resp) => {
                            resp.extensions_mut().insert(MappedError);
                            return Ok(resp);
                        }
                        Err(unchanged) => current = unchanged,
                    }
                }
                Err(current)
            }
        }
    }
}

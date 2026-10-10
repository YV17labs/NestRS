//! Mount-time composition of the response-side layer pools for one HTTP
//! route: exception-filters, filters, interceptors.
//!
//! Guards and pipes run inside [`RouteShaper`] at request time; the
//! response-side families wrap the endpoint, composed here at mount through the
//! same `compose_chain` dedup as every other layer kind.
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
//! The three families compose in **one** call ([`wrap_route_response_layers`]):
//! every nested enum level adds a `Request`-sized slot (~500 B) to the route's
//! future, which poem boxes per request for every route, bare included.
//!
//! [`RouteShaper`]: crate::dispatch::RouteShaper

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, refuse_site, resolve_global_layers,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, UnresolvedLayerError};
use nest_rs_exception_filters::{ExceptionFilterErased, ExceptionFilterSpecs};
use nest_rs_filters::{Filter, FilterChain, FilterSpecs};
use nest_rs_http::MappedError;
use nest_rs_interceptors::{Interceptor, InterceptorChain, InterceptorSpecs};
use poem::endpoint::BoxEndpoint;
use poem::{Endpoint, EndpointExt, Request, Response};

use crate::dispatch::route_shaper::refused_route;
use crate::dispatch::scoped_spec::{
    ScopedExceptionFilterSpec, ScopedFilterSpec, ScopedInterceptorSpec, report_unresolved,
    resolve_scoped,
};

/// One HTTP route's endpoint as it leaves [`wrap_route_response_layers`]:
/// still the macro-emitted handler type, or behind the one box its
/// response-side layer stack composes over.
pub enum RouteEndpoint<E> {
    /// No response-side layer reached this route — the endpoint passes
    /// through untouched; poem's `RouteMethod` boxes it once at mount.
    Plain(E),
    /// At least one interceptor / filter / exception-filter — the composed
    /// stack (interceptors outside filters outside the typed catches)
    /// executes behind a single boxed endpoint.
    Layered(BoxEndpoint<'static, Response>),
}

impl<E> Endpoint for RouteEndpoint<E>
where
    E: Endpoint<Output = Response> + 'static,
{
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        match self {
            Self::Plain(ep) => ep.call(req).await,
            Self::Layered(ep) => ep.call(req).await,
        }
    }
}

/// Compose the route-scoped response-side stack around `endpoint` — the
/// exception-filter pool (typed catches, innermost), the controller / method
/// filter survivors, then the controller / method interceptor survivors
/// (outermost; first-listed outermost within each family). Global
/// interceptors / filters participate in the dedup only — they execute at
/// the transport edge. Called by the `#[routes]` macro at mount time; with
/// every chain empty the endpoint passes through untouched.
///
/// A declared layer that does not resolve refuses the boot through the
/// container, and the route answers an opaque `500` in the meantime.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one chain a decorator collected; a struct would only rename them"
)]
pub fn wrap_route_response_layers<E>(
    container: &Container,
    endpoint: E,
    controller_exception_filters: &[ScopedExceptionFilterSpec],
    method_exception_filters: &[ScopedExceptionFilterSpec],
    controller_filters: &[ScopedFilterSpec],
    method_filters: &[ScopedFilterSpec],
    controller_interceptors: &[ScopedInterceptorSpec],
    method_interceptors: &[ScopedInterceptorSpec],
    route_label: &str,
) -> RouteEndpoint<E>
where
    E: Endpoint<Output = Response> + 'static,
{
    let composed = compose_exception_filters(
        container,
        controller_exception_filters,
        method_exception_filters,
        route_label,
    )
    .and_then(|exception_filters| {
        let filters =
            compose_route_filters(container, controller_filters, method_filters, route_label)?;
        let interceptors = compose_route_interceptors(
            container,
            controller_interceptors,
            method_interceptors,
            route_label,
        )?;
        Ok((exception_filters, filters, interceptors))
    });
    let (exception_filters, filters, interceptors) = match composed {
        Ok(chains) => chains,
        Err(unresolved) => {
            report_unresolved(&unresolved);
            refuse_site(container, unresolved.into());
            return RouteEndpoint::Layered(poem::endpoint::make_sync(|_| refused_route()).boxed());
        }
    };
    if exception_filters.is_empty() && filters.is_empty() && interceptors.is_empty() {
        return RouteEndpoint::Plain(endpoint);
    }
    // An empty stage inside a non-empty stack is a cheap pass-through branch
    // in its runner — boxing once here beats naming all eight stage
    // combinations.
    RouteEndpoint::Layered(
        InterceptorChain::new(
            FilterChain::new(
                ExceptionFiltersEndpoint {
                    inner: endpoint,
                    chain: exception_filters,
                },
                filters,
            ),
            interceptors,
        )
        .boxed(),
    )
}

/// Compose the full exception-filter pool (global + controller + method,
/// deduped) — every scope executes at the route site, closest to the
/// handler, so a typed catch gets the error before a generic `Filter` maps
/// it away.
fn compose_exception_filters(
    container: &Container,
    controller: &[ScopedExceptionFilterSpec],
    method: &[ScopedExceptionFilterSpec],
    route_label: &str,
) -> Result<Vec<ResolvedLayer<dyn ExceptionFilterErased>>, UnresolvedLayerError> {
    let global = resolve_global_layers::<ExceptionFilterSpecs>(container);
    let controller = resolve_scoped(container, controller, LayerSite::Host, route_label)?;
    let method = resolve_scoped(container, method, LayerSite::Method, route_label)?;
    Ok(compose_chain::<dyn ExceptionFilterErased>(
        dedup_bucket(global),
        controller,
        method,
        &[],
        route_label,
    ))
}

/// Compose the route-scoped filter survivors: the full chain (global +
/// controller + method) is composed for dedup; only controller / method
/// survivors run here — global filters execute at the transport edge.
fn compose_route_filters(
    container: &Container,
    controller: &[ScopedFilterSpec],
    method: &[ScopedFilterSpec],
    route_label: &str,
) -> Result<Vec<ResolvedLayer<dyn Filter>>, UnresolvedLayerError> {
    let global = dedup_bucket(resolve_global_layers::<FilterSpecs>(container));
    let controller = resolve_scoped(container, controller, LayerSite::Host, route_label)?;
    let method = resolve_scoped(container, method, LayerSite::Method, route_label)?;
    let chain = compose_chain::<dyn Filter>(global, controller, method, &[], route_label);
    Ok(chain
        .into_iter()
        .filter(|e| e.source != LayerSite::Global)
        .collect())
}

/// Compose the route-scoped interceptor survivors — same site rule as
/// [`compose_route_filters`]. Intra-global duplicates are dropped silently
/// by `dedup_bucket`: the transport edge (the site that executes the global
/// sub-chain) already warned once.
fn compose_route_interceptors(
    container: &Container,
    controller: &[ScopedInterceptorSpec],
    method: &[ScopedInterceptorSpec],
    route_label: &str,
) -> Result<Vec<ResolvedLayer<dyn Interceptor>>, UnresolvedLayerError> {
    let global = dedup_bucket(resolve_global_layers::<InterceptorSpecs>(container));
    let controller = resolve_scoped(container, controller, LayerSite::Host, route_label)?;
    let method = resolve_scoped(container, method, LayerSite::Method, route_label)?;
    let chain = compose_chain::<dyn Interceptor>(global, controller, method, &[], route_label);
    Ok(chain
        .into_iter()
        .filter(|e| e.source != LayerSite::Global)
        .collect())
}

/// Runs the deduped exception-filter chain on the error path: first matching
/// filter wins, the rest of the chain is skipped. A mapped response is
/// tagged [`MappedError`] so the ambient transaction rolls back — the
/// handler failed; a typed catch shapes the client answer, it does not bless
/// the handler's writes.
struct ExceptionFiltersEndpoint<E> {
    inner: E,
    chain: Vec<ResolvedLayer<dyn ExceptionFilterErased>>,
}

impl<E> Endpoint for ExceptionFiltersEndpoint<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        match self.inner.call(req).await {
            Ok(resp) => Ok(resp),
            Err(err) => {
                let mut current = err;
                for entry in &self.chain {
                    // `as_ref()`: dispatch on the erased filter — the
                    // `ExceptionFilterErased for Arc<T>` blanket would nest a
                    // second boxed future around the call.
                    match entry.layer.as_ref().try_catch(current).await {
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

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::__private::take_site_refusal;
    use nest_rs_core::LayerSpec;
    use nest_rs_testing::LogCapture;
    use poem::http::StatusCode;

    use super::*;

    /// A layer no imported module provides: its spec resolves to nothing.
    struct Unprovided;

    fn unprovided<L: ?Sized>() -> LayerSpec<L> {
        LayerSpec::new(TypeId::of::<Unprovided>(), "GhostLayer", |_| None)
    }

    /// The response-side slots `#[routes]` fills for one route.
    #[derive(Clone, Copy, Debug)]
    enum Slot {
        ControllerExceptionFilter,
        MethodExceptionFilter,
        ControllerFilter,
        MethodFilter,
        ControllerInterceptor,
        MethodInterceptor,
    }

    #[tokio::test]
    async fn a_response_layer_no_module_provides_refuses_the_boot_and_answers_500_meanwhile() {
        for slot in [
            Slot::ControllerExceptionFilter,
            Slot::MethodExceptionFilter,
            Slot::ControllerFilter,
            Slot::MethodFilter,
            Slot::ControllerInterceptor,
            Slot::MethodInterceptor,
        ] {
            let logs = LogCapture::install();
            let container = Container::default();
            let mut controller_exception_filters: Vec<ScopedExceptionFilterSpec> = Vec::new();
            let mut method_exception_filters: Vec<ScopedExceptionFilterSpec> = Vec::new();
            let mut controller_filters: Vec<ScopedFilterSpec> = Vec::new();
            let mut method_filters: Vec<ScopedFilterSpec> = Vec::new();
            let mut controller_interceptors: Vec<ScopedInterceptorSpec> = Vec::new();
            let mut method_interceptors: Vec<ScopedInterceptorSpec> = Vec::new();
            match slot {
                Slot::ControllerExceptionFilter => controller_exception_filters.push(unprovided()),
                Slot::MethodExceptionFilter => method_exception_filters.push(unprovided()),
                Slot::ControllerFilter => controller_filters.push(unprovided()),
                Slot::MethodFilter => method_filters.push(unprovided()),
                Slot::ControllerInterceptor => controller_interceptors.push(unprovided()),
                Slot::MethodInterceptor => method_interceptors.push(unprovided()),
            }

            let route = wrap_route_response_layers(
                &container,
                poem::endpoint::make_sync(|_| Response::builder().body("the handler ran")),
                &controller_exception_filters,
                &method_exception_filters,
                &controller_filters,
                &method_filters,
                &controller_interceptors,
                &method_interceptors,
                "get /x",
            );
            let response = route
                .call(Request::default())
                .await
                .expect("a refused route answers");
            assert_eq!(
                response.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{slot:?}: the route would run without the layer it declares",
            );

            let refusal = take_site_refusal(&container)
                .map(|refusal| format!("{refusal:#}"))
                .unwrap_or_else(|| panic!("{slot:?}: the boot finds no refusal to fail on"));
            assert!(
                refusal.contains("GhostLayer") && refusal.contains("get /x"),
                "{slot:?}: the refusal names the layer and the route: {refusal}",
            );
            let event = logs.expect_one(
                "nest_rs::layers",
                "a layer the site declares is provided by no imported module",
            );
            assert_eq!(event.level, "error", "{slot:?}");
            assert_eq!(
                event.field("layer").as_deref(),
                Some("GhostLayer"),
                "{slot:?}"
            );
            assert_eq!(event.field("site").as_deref(), Some("get /x"), "{slot:?}");
        }
    }
}

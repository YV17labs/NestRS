//! A route on the pipeline: the site `#[routes]` declares for each handler in
//! a `static`, mounted on the route table with the chains its container
//! composes, its global pipes and the unit of work it runs inside.

use std::any::TypeId;
use std::future::Future;
use std::sync::Arc;

use http::Method;
use nest_rs_core::__private::{
    GlobalLayers, GuardStage, HandlerDeclaration, MetaLevels, ResolvedLayer, Site, SiteDeclaration,
    SiteLayers, compose_chain, declare_handler, declare_site, dedup_bucket, log_effective_chain,
    report_unresolved, resolve_scoped,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, Handler, Posture};
use nest_rs_guards::__private::{
    boot_validate_guards, resolve_global_guards, validate_route_chain,
};
use nest_rs_guards::{Guard, GuardSpec};
use nest_rs_pipes::PipeSpec;

use crate::deprecation::deprecated_route;
use crate::endpoint::BoxEndpoint;
use crate::error::{Result, RouteError};
use crate::global_pipes::GlobalPipes;
use crate::guard_stage::{HttpGuardStage, StageKind};
use crate::request::Request;
use crate::response::Response;
use crate::routes::HttpRoutes;
use crate::shaper::GateFn;
use crate::site::{SiteEndpoint, SiteTail, UnitPolicy};

/// What `#[routes]` makes of a handler method: its parameters read off the
/// request it borrows — the body taken, the head left to the layers that read
/// the unit once it answered — then the method called and its answer shaped.
pub trait RouteWrapper: Send + Sync + 'static {
    /// Answer `req`.
    fn call(&self, req: &mut Request) -> impl Future<Output = Result<Response>> + Send;
}

/// What a route runs last: its global pipes over the body, then the handler
/// and its response shaper.
struct RouteTail<W> {
    handler: &'static Handler,
    pipes: GlobalPipes,
    wrapper: W,
}

impl<W: RouteWrapper> SiteTail for RouteTail<W> {
    async fn answer(&self, req: &mut Request) -> Result<Response> {
        if !self.pipes.is_empty() {
            // Boxed on purpose: inlining the pipes' large state machine would
            // weigh on every route's future, which the table boxes per request.
            Box::pin(self.pipes.apply(self.handler, req)).await?;
        }
        self.wrapper.call(req).await
    }
}

/// What a controller registers on the table by hand, outside `#[routes]`: a
/// route all the same, run as one — the app's guard pool, its exception
/// filters and its interceptors around the endpoint, inside a unit of work.
pub(crate) static HAND_ROUTE: Handler = declare_handler(HandlerDeclaration {
    unit: crate::unit::REQUEST,
    host: TypeId::of::<HttpRoutes>(),
    host_name: "HttpRoutes",
    method: "route",
    address: "*",
    posture: Posture::Gated,
    meta: MetaLevels::default,
});

/// The one site of every route a controller registers by hand.
pub(crate) static HAND_ROUTE_SITE: Site = declare_site(
    &HAND_ROUTE,
    SiteDeclaration {
        layers: SiteLayers::default,
        global: GlobalLayers::ALL,
        guard_stage: Some(hand_route_guard_stage),
        class_gate: None,
    },
);

/// A hand-registered route's guard stage: the app's pool, as a `#[routes]`
/// route declaring no guard of its own runs it.
fn hand_route_guard_stage(
    container: &Container,
    handler: &Handler,
) -> anyhow::Result<Option<Arc<dyn GuardStage>>> {
    route_guard_stage(
        container,
        handler,
        RouteGuards {
            host: Vec::new(),
            method: Vec::new(),
            force: Vec::new(),
            gates: Vec::new(),
            masked: false,
        },
    )
}

/// The global pipes a route declares, as `#[routes]` collects them.
pub struct RoutePipes {
    /// `#[use_pipes]` on the controller.
    pub host: Vec<PipeSpec>,
    /// `#[use_pipes]` beside the handler.
    pub method: Vec<PipeSpec>,
    /// `#[no_pipes]`: none runs, the app's pool included.
    pub none: bool,
}

/// What a route's guard stage composes, as `#[routes]` collects it.
pub struct RouteGuards {
    /// `#[use_guards]` on the controller.
    pub host: Vec<GuardSpec>,
    /// `#[use_guards]` beside the handler, `#[force_guards]` included.
    pub method: Vec<GuardSpec>,
    /// `#[force_guards]`: the guards the method re-runs.
    pub force: Vec<TypeId>,
    /// The posture's class gates, selected by type off what the route
    /// declares.
    pub gates: Vec<GateFn>,
    /// Whether a response shaper masks the route's reply.
    pub masked: bool,
}

/// The guard stage of the route `handler` declares: the app's pool, then the
/// controller's and the method's guards, each once, then its class gates —
/// `None` when it runs none. A route whose method scope declares guards is
/// validated whole; the controller's mount validated the rest
/// ([`mount_controller`]).
pub fn route_guard_stage(
    container: &Container,
    handler: &Handler,
    declared: RouteGuards,
) -> anyhow::Result<Option<Arc<dyn GuardStage>>> {
    let label = route_label(handler);
    let host = resolve_scoped(container, &declared.host, LayerSite::Host, &label)
        .inspect_err(report_unresolved)?;
    let method = resolve_scoped(container, &declared.method, LayerSite::Method, &label)
        .inspect_err(report_unresolved)?;
    let chain = compose_chain::<dyn Guard>(
        dedup_bucket(resolve_global_guards(container)),
        host,
        method,
        &declared.force,
        &label,
    );
    if !declared.method.is_empty() || !declared.force.is_empty() {
        validate_route_chain(&label, &chain).map_err(anyhow::Error::msg)?;
    }
    log_effective_chain(&label, "guards", &chain);
    Ok(stage(chain, declared.gates, declared.masked))
}

/// The stage running `chain` then `gates`, or `None` when both are empty.
fn stage(
    chain: Vec<ResolvedLayer<dyn Guard>>,
    gates: Vec<GateFn>,
    masked: bool,
) -> Option<Arc<dyn GuardStage>> {
    if chain.is_empty() && gates.is_empty() {
        return None;
    }
    Some(Arc::new(HttpGuardStage::new(
        chain,
        gates,
        StageKind::Route { unmasked: !masked },
    )))
}

/// A route as a boot error and a chain's log line name it.
fn route_label(handler: &Handler) -> String {
    format!("{}::{}", handler.host_name(), handler.method())
}

/// Check the controller `controller`'s guards once, before its routes mount:
/// the app's pool and its own, composed as a route without guards of its own
/// runs them. A refusal fails the boot naming the controller.
pub fn mount_controller(
    container: &Container,
    routes: &mut HttpRoutes,
    controller: &'static str,
    guards: &[GuardSpec],
) {
    if let Err(refused) = boot_validate_guards(container, guards, controller) {
        routes.refuse(RouteError::Layers {
            site: format!("the controller {controller}"),
            source: refused.into(),
        });
    }
}

/// Mount `wrapper`, the handler of `site`, at `method` `path` on `routes`: the
/// chains its container composes, its global pipes and the unit of work it
/// runs inside, the `Deprecation` header `deprecated` states on every answer.
/// A chain that does not compose fails the boot naming the route, and nothing
/// serves it.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one fact `#[routes]` collected for one route; a struct would only \
              rename them"
)]
pub fn mount_route<W: RouteWrapper>(
    container: &Container,
    routes: &mut HttpRoutes,
    method: Method,
    path: &str,
    site: &'static Site,
    pipes: RoutePipes,
    wrapper: W,
    deprecated: Option<&'static str>,
) {
    let handler = site.handler();
    let composed = site.chains(container).and_then(|chains| {
        let pipes = match pipes.none {
            true => GlobalPipes::none(),
            false => {
                GlobalPipes::resolve(container, &route_label(handler), &pipes.host, &pipes.method)?
            }
        };
        Ok((chains, pipes))
    });
    let (chains, pipes) = match composed {
        Ok(composed) => composed,
        Err(refused) => {
            let site = format!(
                "the route {method} {} of {}",
                routes.served(path),
                routes.registrant()
            );
            routes.refuse(RouteError::Layers {
                site,
                source: refused.into(),
            });
            return;
        }
    };
    let unit = routes
        .unit_of_work()
        .map(|unit| (Arc::clone(unit), UnitPolicy::ByMethod));
    let tail = RouteTail {
        handler,
        pipes,
        wrapper,
    };
    let route = SiteEndpoint::new(handler, chains, unit, tail);
    let route = match deprecated {
        Some(header) => BoxEndpoint::new(deprecated_route(route, header)),
        None => BoxEndpoint::new(route),
    };
    routes.site(method, path, route);
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::compose_site;

    use super::*;
    use crate::endpoint::Endpoint;
    use crate::shaper::shaped;
    use crate::testing::{Holding, get};

    struct Routes;

    static ROUTE: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::REQUEST,
        host: TypeId::of::<Routes>(),
        host_name: "Routes",
        method: "route",
        address: "/x",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    /// What a route's site adds to the future of a handler holding `N` bytes:
    /// every stage the edge runs between the table and the handler, as
    /// [`mount_route`] composes them.
    fn added_by_the_site<const N: usize>() -> usize {
        let chains = compose_site(
            &Container::default(),
            &ROUTE,
            &SiteDeclaration {
                layers: SiteLayers::default,
                global: GlobalLayers::ALL,
                guard_stage: None,
                class_gate: None,
            },
            None,
        )
        .expect("the site composes");
        let tail = RouteTail {
            handler: &ROUTE,
            pipes: GlobalPipes::none(),
            wrapper: shaped(Holding::<N>, None, Some("POST /x")),
        };
        let route = deprecated_route(SiteEndpoint::new(&ROUTE, chains, None, tail), "true");
        let handler = size_of_val(&RouteWrapper::call(&Holding::<N>, &mut get()));
        size_of_val(&route.call(get())) - handler
    }

    /// A debug build reserves every copy of a future a frame holds on the
    /// thread polling it: a site copying its handler's would multiply the
    /// stack a request needs by the stages it runs.
    #[test]
    fn a_routes_site_holds_its_handlers_future_once() {
        let light = added_by_the_site::<1024>();
        let heavy = added_by_the_site::<16384>();
        assert_eq!(
            light, heavy,
            "a route's site copies its handler's future: it adds {light} bytes to a 1 KiB one, \
             {heavy} to a 16 KiB one",
        );
    }
}

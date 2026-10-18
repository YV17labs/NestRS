//! A route on the pipeline: the site `#[routes]` declares for each handler in
//! a `static`, mounted on the route table with the chains its container
//! composes, its global pipes and the unit of work it runs inside.

use std::any::TypeId;
use std::sync::Arc;

use http::Method;
use nest_rs_core::__private::{
    GuardStage, ResolvedLayer, Site, compose_chain, dedup_bucket, log_effective_chain,
    report_unresolved, resolve_scoped,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, Handler};
use nest_rs_guards::__private::{
    boot_validate_guards, resolve_global_guards, validate_route_chain,
};
use nest_rs_guards::{Guard, GuardSpec};
use nest_rs_pipes::PipeSpec;

use crate::deprecation::deprecated_route;
use crate::endpoint::Endpoint;
use crate::error::{Result, RouteError};
use crate::global_pipes::GlobalPipes;
use crate::guard_stage::{HttpGuardStage, StageKind};
use crate::request::Request;
use crate::response::Response;
use crate::routes::HttpRoutes;
use crate::shaper::GateFn;
use crate::site::{SiteEndpoint, SiteTail, UnitPolicy};

/// A route mounted on the pipeline.
pub(crate) type RouteSite<E> = SiteEndpoint<RouteTail<E>>;

/// What a route runs last: its global pipes over the body, then the handler
/// and its response shaper.
pub(crate) struct RouteTail<E> {
    handler: &'static Handler,
    pipes: GlobalPipes,
    endpoint: E,
}

impl<E: Endpoint> SiteTail for RouteTail<E> {
    async fn answer(&self, mut req: Request) -> Result<Response> {
        if !self.pipes.is_empty() {
            // Boxed on purpose: inlining the pipes' large state machine would
            // weigh on every route's future, which the table boxes per request.
            Box::pin(self.pipes.apply(self.handler, &mut req)).await?;
        }
        self.endpoint.call(req).await
    }
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

/// Mount `endpoint`, the handler of `site`, at `method` `path` on `routes`: the
/// chains its container composes, its global pipes and the unit of work it
/// runs inside, the `Deprecation` header `deprecated` states on every answer.
/// A chain that does not compose fails the boot naming the route, and nothing
/// serves it.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one fact `#[routes]` collected for one route; a struct would only \
              rename them"
)]
pub fn mount_route<E: Endpoint>(
    container: &Container,
    routes: &mut HttpRoutes,
    method: Method,
    path: &str,
    site: &'static Site,
    pipes: RoutePipes,
    endpoint: E,
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
        endpoint,
    };
    let route: RouteSite<E> = SiteEndpoint::new(handler, chains, unit, tail);
    match deprecated {
        Some(header) => routes.route(method, path, deprecated_route(route, header)),
        None => routes.route(method, path, route),
    };
}

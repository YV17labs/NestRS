//! Transport integration — types and helpers the three shaper macros emit
//! at the start of every handler.
//!
//! ## HTTP — per-route interceptor wrapper
//!
//! Each route gets its own [`LayersRouteInterceptor`] instance, baked at
//! mount time with the per-route guard / pipe specs the macro collected from
//! `#[use_guards]` / `#[use_pipes]` / `#[force_guards]`. Wrapped as the
//! outermost handler layer so the global chain runs before the handler.
//!
//! Note: `#[public]` is NOT a framework-level skip — the macro attaches a
//! [`Public`](nest_rs_core::Public) marker via the same metadata mechanism
//! as `#[meta(...)]`, and individual guards decide whether to honor it.
//!
//! ## GraphQL / WS — inline calls
//!
//! The `#[resolver]` and `#[messages]` macros emit a call to
//! [`run_layered_graphql_chain`] / [`run_layered_ws_chain`] at the start
//! of every handler method.

use std::any::TypeId;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use nest_rs_core::{Container, Layer, RequestScope};
use nest_rs_graphql::async_graphql::{
    Context as GraphqlContext, Error as GraphqlError, ErrorExtensions,
};
use nest_rs_http::poem::http::StatusCode;
use nest_rs_http::poem::{Body, Request, Response, Result};
use nest_rs_interceptors::{Interceptor, Next};
use nest_rs_pipes::GlobalPipe;
use nest_rs_ws::WsClient;
use serde_json::Value;

use crate::Guard;
use crate::denial::Denial;
use crate::layer_chain::{LayerSource, ResolvedLayer, compose_chain};
use crate::registry::{ExceptionFilterSpecs, GuardSpecs, PipeSpecs};
use nest_rs_exception_filters::ExceptionFilterErased;

/// A per-route guard the macro emitted from `#[use_guards(X)]`. Carries
/// the `TypeId` so dedup sees it as the same key as the global registration.
pub struct RouteLayerSpec<L: ?Sized> {
    pub type_id: TypeId,
    pub name: &'static str,
    pub resolve: fn(&Container) -> Option<Arc<L>>,
}

/// A guard spec for a specific route — the macro-emitted form.
pub type RouteGuardSpec = RouteLayerSpec<dyn Guard>;
/// A pipe spec for a specific route — used when the route declares
/// `#[use_pipes(...)]` (rare; most pipes are global).
pub type RoutePipeSpec = RouteLayerSpec<dyn GlobalPipe>;
/// An exception-filter spec for a specific route — used when the route or its
/// controller declares `#[use_exception_filters(...)]`.
pub type RouteExceptionFilterSpec = RouteLayerSpec<dyn ExceptionFilterErased>;

/// HTTP per-route interceptor.
///
/// Constructed by the `#[routes]` macro at mount time. Resolves the global
/// + per-route guard / pipe chain on first request, dedups by `TypeId`,
///   caches, runs every layer in order — no `#[public]` skip, no
///   category-based reordering. Guards decide what `#[public]` means for
///   them via the [`Public`](nest_rs_core::Public) marker attached as
///   request data.
///
/// Itself a [`Layer`] so it satisfies the `Interceptor: Layer` bound; this
/// per-route interceptor never participates in the dedup pass (it *is* the
/// dedup pass), so the default `priority()` / `name()` are correct.
pub struct LayersRouteInterceptor {
    route_label: &'static str,
    controller_guards: Vec<RouteGuardSpec>,
    method_guards: Vec<RouteGuardSpec>,
    force_guards: Vec<TypeId>,
    controller_pipes: Vec<RoutePipeSpec>,
    method_pipes: Vec<RoutePipeSpec>,
    no_pipes: bool,
    controller_exception_filters: Vec<RouteExceptionFilterSpec>,
    method_exception_filters: Vec<RouteExceptionFilterSpec>,
    cached_guards: OnceLock<Vec<ResolvedLayer<dyn Guard>>>,
    cached_pipes: OnceLock<Vec<ResolvedLayer<dyn GlobalPipe>>>,
    cached_exception_filters: OnceLock<Vec<ResolvedLayer<dyn ExceptionFilterErased>>>,
}

impl LayersRouteInterceptor {
    // Macros emit this — a parameter struct would only add indirection at the
    // call sites the user never reads.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        route_label: &'static str,
        controller_guards: Vec<RouteGuardSpec>,
        method_guards: Vec<RouteGuardSpec>,
        force_guards: Vec<TypeId>,
        controller_pipes: Vec<RoutePipeSpec>,
        method_pipes: Vec<RoutePipeSpec>,
        no_pipes: bool,
        controller_exception_filters: Vec<RouteExceptionFilterSpec>,
        method_exception_filters: Vec<RouteExceptionFilterSpec>,
    ) -> Self {
        Self {
            route_label,
            controller_guards,
            method_guards,
            force_guards,
            controller_pipes,
            method_pipes,
            no_pipes,
            controller_exception_filters,
            method_exception_filters,
            cached_guards: OnceLock::new(),
            cached_pipes: OnceLock::new(),
            cached_exception_filters: OnceLock::new(),
        }
    }

    fn resolve_guards(&self, container: &Container) -> Vec<ResolvedLayer<dyn Guard>> {
        let mut global: Vec<ResolvedLayer<dyn Guard>> = Vec::new();
        if let Some(specs) = container.get::<GuardSpecs>() {
            for spec in &specs.0 {
                if let Some(layer) = spec.resolve(container) {
                    global.push(ResolvedLayer {
                        type_id: spec.type_id,
                        name: spec.name,
                        source: LayerSource::Global,
                        layer,
                    });
                }
            }
        }
        let controller = resolve_specs(container, &self.controller_guards, LayerSource::Controller);
        let method = resolve_specs(container, &self.method_guards, LayerSource::Method);
        let chain = compose_chain::<dyn Guard>(
            global,
            controller,
            method,
            &self.force_guards,
            self.route_label,
        );
        log_effective_chain(self.route_label, "guards", &chain);
        // Globals are run transport-level by
        // [`crate::builder::GlobalGuardsHttpInterceptor`]; the per-route
        // chain only executes the controller/method scopes. Globals stay in
        // the chain *for dedup*: a controller/method declaration with the
        // same TypeId is skipped here so it doesn't double-fire.
        chain
            .into_iter()
            .filter(|entry| entry.source != LayerSource::Global)
            .collect()
    }

    fn resolve_pipes(&self, container: &Container) -> Vec<ResolvedLayer<dyn GlobalPipe>> {
        // `#[no_pipes]` skips every pipe — globals, controller, method.
        if self.no_pipes {
            return Vec::new();
        }
        let mut global: Vec<ResolvedLayer<dyn GlobalPipe>> = Vec::new();
        if let Some(specs) = container.get::<PipeSpecs>() {
            for spec in &specs.0 {
                if let Some(layer) = spec.resolve(container) {
                    global.push(ResolvedLayer {
                        type_id: spec.type_id,
                        name: spec.name,
                        source: LayerSource::Global,
                        layer,
                    });
                }
            }
        }
        let controller = resolve_specs(container, &self.controller_pipes, LayerSource::Controller);
        let method = resolve_specs(container, &self.method_pipes, LayerSource::Method);
        let chain =
            compose_chain::<dyn GlobalPipe>(global, controller, method, &[], self.route_label);
        log_effective_chain(self.route_label, "pipes", &chain);
        chain
    }

    fn resolve_exception_filters(
        &self,
        container: &Container,
    ) -> Vec<ResolvedLayer<dyn ExceptionFilterErased>> {
        let mut global: Vec<ResolvedLayer<dyn ExceptionFilterErased>> = Vec::new();
        if let Some(specs) = container.get::<ExceptionFilterSpecs>() {
            for spec in &specs.0 {
                if let Some(layer) = spec.resolve(container) {
                    global.push(ResolvedLayer {
                        type_id: spec.type_id,
                        name: spec.name,
                        source: LayerSource::Global,
                        layer,
                    });
                }
            }
        }
        let controller = resolve_specs(
            container,
            &self.controller_exception_filters,
            LayerSource::Controller,
        );
        let method = resolve_specs(
            container,
            &self.method_exception_filters,
            LayerSource::Method,
        );
        let chain = compose_chain::<dyn ExceptionFilterErased>(
            global,
            controller,
            method,
            &[],
            self.route_label,
        );
        log_effective_chain(self.route_label, "exception_filters", &chain);
        chain
    }
}

fn resolve_specs<L: ?Sized>(
    container: &Container,
    specs: &[RouteLayerSpec<L>],
    source: LayerSource,
) -> Vec<ResolvedLayer<L>> {
    specs
        .iter()
        .filter_map(|spec| {
            (spec.resolve)(container).map(|layer| ResolvedLayer {
                type_id: spec.type_id,
                name: spec.name,
                source,
                layer,
            })
        })
        .collect()
}

fn log_effective_chain<L: Layer + ?Sized>(route: &str, kind: &str, chain: &[ResolvedLayer<L>]) {
    if chain.is_empty() {
        return;
    }
    let entries: Vec<String> = chain
        .iter()
        .map(|e| format!("{} ({})", e.name, e.source.label()))
        .collect();
    tracing::debug!(
        target: "nest_rs::layers",
        route,
        kind,
        chain = entries.join(", ").as_str(),
        "effective layer chain",
    );
}

impl Layer for LayersRouteInterceptor {}

#[async_trait]
impl Interceptor for LayersRouteInterceptor {
    async fn intercept(&self, mut req: Request, next: Next<'_>) -> Result<Response> {
        let scope = req.extensions().get::<Arc<RequestScope>>().cloned();
        let Some(scope) = scope else {
            return next.run(req).await;
        };
        let container = scope.root();

        let guards = self
            .cached_guards
            .get_or_init(|| self.resolve_guards(container));
        for entry in guards {
            if let Err(denial) = entry.layer.check_http(&mut req).await {
                return Ok(denial_to_http_response(denial));
            }
        }

        let pipes = self
            .cached_pipes
            .get_or_init(|| self.resolve_pipes(container));
        if !pipes.is_empty() {
            apply_body_pipes(&mut req, pipes).await?;
        }

        let filters = self
            .cached_exception_filters
            .get_or_init(|| self.resolve_exception_filters(container));

        match next.run(req).await {
            Ok(resp) => Ok(resp),
            Err(err) if filters.is_empty() => Err(err),
            Err(err) => {
                let mut current = err;
                for entry in filters {
                    match entry.layer.try_catch(current).await {
                        Ok(resp) => return Ok(resp),
                        Err(unchanged) => current = unchanged,
                    }
                }
                Err(current)
            }
        }
    }
}

/// Read the JSON body, run every pipe in order, write the rewritten body
/// back into the request. No-op when the body is missing / not JSON / no
/// pipe rejects.
async fn apply_body_pipes(
    req: &mut Request,
    pipes: &[ResolvedLayer<dyn GlobalPipe>],
) -> Result<()> {
    let content_type = req
        .headers()
        .get(nest_rs_http::poem::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !content_type.contains("json") {
        return Ok(());
    }
    let body = req.take_body();
    let bytes = match body.into_bytes().await {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(target: "nest_rs::layers", error = %err, "global pipe: failed to read body");
            return Ok(());
        }
    };
    if bytes.is_empty() {
        return Ok(());
    }
    let mut value: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(target: "nest_rs::layers", error = %err, "global pipe: body is not valid JSON");
            req.set_body(Body::from_bytes(bytes));
            return Ok(());
        }
    };
    for entry in pipes {
        if let Err(err) = entry.layer.transform_body(&mut value) {
            let mut body = serde_json::json!({
                "statusCode": 400,
                "error": "Bad Request",
                "message": err.message(),
            });
            if let Some(details) = err.into_details() {
                body["details"] = details;
            }
            let resp = Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .content_type("application/json")
                .body(serde_json::to_vec(&body).unwrap_or_default());
            return Err(nest_rs_http::poem::Error::from_response(resp));
        }
    }
    let rewritten = serde_json::to_vec(&value).unwrap_or_default();
    req.set_body(Body::from_bytes(rewritten.into()));
    Ok(())
}

/// GraphQL shaper helper. Called by `#[resolver]` at the start of every
/// resolver method. Dedups per-resolver guards against the global chain.
pub async fn run_layered_graphql_chain(
    ctx: &GraphqlContext<'_>,
    container: &Container,
    controller_guards: &[RouteGuardSpec],
    method_guards: &[RouteGuardSpec],
    force_guards: &[TypeId],
    route_label: &str,
) -> std::result::Result<(), GraphqlError> {
    let mut global: Vec<ResolvedLayer<dyn Guard>> = Vec::new();
    if let Some(specs) = container.get::<GuardSpecs>() {
        for spec in &specs.0 {
            if let Some(layer) = spec.resolve(container) {
                global.push(ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source: LayerSource::Global,
                    layer,
                });
            }
        }
    }
    let controller = resolve_specs(container, controller_guards, LayerSource::Controller);
    let method = resolve_specs(container, method_guards, LayerSource::Method);
    let chain = compose_chain::<dyn Guard>(global, controller, method, force_guards, route_label);
    for entry in &chain {
        if let Err(denial) = entry.layer.check_graphql(ctx).await {
            return Err(denial_to_graphql_error(denial));
        }
    }
    Ok(())
}

/// WS shaper helper. Called by `#[messages]` at the start of every
/// `#[subscribe_message]` handler. Dedups per-message guards against the
/// global chain.
#[allow(clippy::too_many_arguments)]
pub async fn run_layered_ws_chain(
    client: &WsClient,
    event: &str,
    data: &Value,
    container: &Container,
    controller_guards: &[RouteGuardSpec],
    method_guards: &[RouteGuardSpec],
    force_guards: &[TypeId],
    route_label: &str,
) -> std::result::Result<(), String> {
    let mut global: Vec<ResolvedLayer<dyn Guard>> = Vec::new();
    if let Some(specs) = container.get::<GuardSpecs>() {
        for spec in &specs.0 {
            if let Some(layer) = spec.resolve(container) {
                global.push(ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source: LayerSource::Global,
                    layer,
                });
            }
        }
    }
    let controller = resolve_specs(container, controller_guards, LayerSource::Controller);
    let method = resolve_specs(container, method_guards, LayerSource::Method);
    let chain = compose_chain::<dyn Guard>(global, controller, method, force_guards, route_label);
    for entry in &chain {
        if let Err(denial) = entry.layer.check_ws_message(client, event, data).await {
            return Err(denial.message().to_owned());
        }
    }
    Ok(())
}

/// Convert a transport-agnostic [`Denial`] to a poem [`Response`].
pub fn denial_to_http_response(denial: Denial) -> Response {
    let status = match denial.http_status() {
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        429 => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut builder = Response::builder().status(status);
    if let Denial::RateLimited {
        retry_after_secs, ..
    } = &denial
    {
        builder = builder.header("Retry-After", retry_after_secs.to_string());
    }
    builder.body(Body::from_string(denial.message().to_owned()))
}

/// Convert a [`Denial`] to an async-graphql error frame.
pub fn denial_to_graphql_error(denial: Denial) -> GraphqlError {
    let code = match denial.http_status() {
        401 => "UNAUTHENTICATED",
        403 => "FORBIDDEN",
        429 => "RATE_LIMITED",
        _ => "INTERNAL",
    };
    GraphqlError::new(denial.message().to_owned()).extend_with(|_, e| e.set("code", code))
}

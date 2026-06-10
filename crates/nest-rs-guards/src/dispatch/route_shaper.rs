//! [`RouteShaper`] — the HTTP per-route shaper. Implements `Interceptor`
//! so the `#[routes]` macro can wrap it around each route's endpoint at
//! mount time. Orchestrates the request-side layer families — guards and
//! pipes — at the route scope, deduplicating against the global chain by
//! `TypeId`. The response-side families (exception-filters, filters,
//! interceptors) wrap the endpoint *inside* the shaper — see
//! [`route_layers`](crate::dispatch::route_layers).

use std::any::TypeId;

use async_trait::async_trait;
use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain, dedup_bucket};
use nest_rs_core::{Container, Layer};
use nest_rs_http::poem::http::StatusCode;
use nest_rs_http::poem::{Body, Request, Response, Result};
use nest_rs_interceptors::{Interceptor, Next};
use nest_rs_pipes::GlobalPipe;
use serde_json::Value;

use crate::Guard;
use crate::dispatch::denial_convert::denial_to_http_response;
use crate::dispatch::scoped_spec::{ScopedGuardSpec, ScopedPipeSpec, resolve_specs};
use crate::registry::{GuardSpecs, PipeSpecs};

/// HTTP per-route shaper.
///
/// Constructed by the `#[routes]` macro at mount time with the
/// controller / method scope specs. Resolves the global + per-route
/// chain **eagerly against the mount-time container** (the container is
/// final at `configure`; resolving lazily would only delay surfacing a
/// broken chain to the first request), dedups by `TypeId`, runs every
/// layer in declaration order. No `#[public]` skip — guards decide what
/// `#[public]` means for them via the [`Public`](nest_rs_core::Public)
/// marker attached as request data.
///
/// Implements [`Layer`] only to satisfy the `Interceptor: Layer` bound;
/// the shaper never participates in the dedup pass (it *is* the dedup
/// pass), so default `priority()` / `name()` are correct.
pub struct RouteShaper {
    guards: Vec<ResolvedLayer<dyn Guard>>,
    pipes: Vec<ResolvedLayer<dyn GlobalPipe>>,
}

impl RouteShaper {
    // Macros emit this — a parameter struct would only add indirection at
    // call sites the user never reads.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        container: &Container,
        route_label: &'static str,
        controller_guards: Vec<ScopedGuardSpec>,
        method_guards: Vec<ScopedGuardSpec>,
        force_guards: Vec<TypeId>,
        controller_pipes: Vec<ScopedPipeSpec>,
        method_pipes: Vec<ScopedPipeSpec>,
        no_pipes: bool,
    ) -> Self {
        let guards = resolve_guards(
            container,
            route_label,
            &controller_guards,
            &method_guards,
            &force_guards,
        );
        let pipes = if no_pipes {
            // `#[no_pipes]` skips every pipe — globals, controller, method.
            Vec::new()
        } else {
            resolve_pipes(container, route_label, &controller_pipes, &method_pipes)
        };
        Self { guards, pipes }
    }
}

fn resolve_guards(
    container: &Container,
    route_label: &str,
    controller_guards: &[ScopedGuardSpec],
    method_guards: &[ScopedGuardSpec],
    force_guards: &[TypeId],
) -> Vec<ResolvedLayer<dyn Guard>> {
    let mut global: Vec<ResolvedLayer<dyn Guard>> = Vec::new();
    if let Some(specs) = container.get::<GuardSpecs>() {
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
    let controller = resolve_specs(container, controller_guards, LayerSite::Controller);
    let method = resolve_specs(container, method_guards, LayerSite::Method);
    let chain = compose_chain::<dyn Guard>(
        dedup_bucket(global),
        controller,
        method,
        force_guards,
        route_label,
    );
    log_effective_chain(route_label, "guards", &chain);
    // The shaper is the single execution site for the guard pool on a
    // routed handler: global + controller + method, deduped by `TypeId`
    // (broadest scope wins), run here *after* routing so a guard reads
    // `#[public]`. Self-mounting endpoints (no shaper) get the global
    // chain at the transport edge (`SelfMountGuardWrap`) or in-band
    // (GraphQL operation guard) instead.
    chain
}

fn resolve_pipes(
    container: &Container,
    route_label: &str,
    controller_pipes: &[ScopedPipeSpec],
    method_pipes: &[ScopedPipeSpec],
) -> Vec<ResolvedLayer<dyn GlobalPipe>> {
    let mut global: Vec<ResolvedLayer<dyn GlobalPipe>> = Vec::new();
    if let Some(specs) = container.get::<PipeSpecs>() {
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
    let controller = resolve_specs(container, controller_pipes, LayerSite::Controller);
    let method = resolve_specs(container, method_pipes, LayerSite::Method);
    let chain =
        compose_chain::<dyn GlobalPipe>(dedup_bucket(global), controller, method, &[], route_label);
    log_effective_chain(route_label, "pipes", &chain);
    chain
}

impl Layer for RouteShaper {}

#[async_trait]
impl Interceptor for RouteShaper {
    async fn intercept(&self, mut req: Request, next: Next<'_>) -> Result<Response> {
        for entry in &self.guards {
            if let Err(denial) = entry.layer.check_http(&mut req).await {
                return Ok(denial_to_http_response(denial));
            }
        }

        if !self.pipes.is_empty() {
            apply_body_pipes(&mut req, &self.pipes).await?;
        }

        next.run(req).await
    }
}

pub(super) fn log_effective_chain<L: Layer + ?Sized>(
    route: &str,
    kind: &str,
    chain: &[ResolvedLayer<L>],
) {
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
    let limit = req
        .extensions()
        .get::<nest_rs_http::RawBodyLimit>()
        .map(|l| l.0)
        .unwrap_or(nest_rs_http::RawBody::DEFAULT_LIMIT);
    let body = req.take_body();
    let bytes = match body.into_bytes_limit(limit).await {
        Ok(b) => b,
        Err(nest_rs_http::poem::error::ReadBodyError::PayloadTooLarge) => {
            return Err(nest_rs_http::poem::Error::from_status(
                nest_rs_http::poem::http::StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
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

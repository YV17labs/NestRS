//! [`RouteShaper`] — the HTTP per-route shaper. Implements `Interceptor`
//! so the `#[routes]` macro can wrap it around each route's endpoint at
//! mount time. Orchestrates guards / pipes / exception-filters at the
//! route scope, deduplicating against the global chain by `TypeId`.

use std::any::TypeId;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use nest_rs_core::{Container, Layer, RequestScope};
use nest_rs_exception_filters::ExceptionFilterErased;
use nest_rs_http::poem::http::StatusCode;
use nest_rs_http::poem::{Body, Request, Response, Result};
use nest_rs_interceptors::{Interceptor, Next};
use nest_rs_pipes::GlobalPipe;
use serde_json::Value;

use crate::Guard;
use crate::dispatch::denial_convert::denial_to_http_response;
use crate::dispatch::scoped_spec::{
    ScopedExceptionFilterSpec, ScopedGuardSpec, ScopedPipeSpec, resolve_specs,
};
use crate::layer_chain::{LayerSource, ResolvedLayer, compose_chain};
use crate::registry::{ExceptionFilterSpecs, GuardSpecs, PipeSpecs};

/// HTTP per-route shaper.
///
/// Constructed by the `#[routes]` macro at mount time with the
/// controller / method scope specs. Resolves the global + per-route
/// chain on first request, dedups by `TypeId`, caches in `OnceLock`s,
/// runs every layer in declaration order. No `#[public]` skip — guards
/// decide what `#[public]` means for them via the
/// [`Public`](nest_rs_core::Public) marker attached as request data.
///
/// Implements [`Layer`] only to satisfy the `Interceptor: Layer` bound;
/// the shaper never participates in the dedup pass (it *is* the dedup
/// pass), so default `priority()` / `name()` are correct.
pub struct RouteShaper {
    route_label: &'static str,
    controller_guards: Vec<ScopedGuardSpec>,
    method_guards: Vec<ScopedGuardSpec>,
    force_guards: Vec<TypeId>,
    controller_pipes: Vec<ScopedPipeSpec>,
    method_pipes: Vec<ScopedPipeSpec>,
    no_pipes: bool,
    controller_exception_filters: Vec<ScopedExceptionFilterSpec>,
    method_exception_filters: Vec<ScopedExceptionFilterSpec>,
    cached_guards: OnceLock<Vec<ResolvedLayer<dyn Guard>>>,
    cached_pipes: OnceLock<Vec<ResolvedLayer<dyn GlobalPipe>>>,
    cached_exception_filters: OnceLock<Vec<ResolvedLayer<dyn ExceptionFilterErased>>>,
}

impl RouteShaper {
    // Macros emit this — a parameter struct would only add indirection at
    // call sites the user never reads.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        route_label: &'static str,
        controller_guards: Vec<ScopedGuardSpec>,
        method_guards: Vec<ScopedGuardSpec>,
        force_guards: Vec<TypeId>,
        controller_pipes: Vec<ScopedPipeSpec>,
        method_pipes: Vec<ScopedPipeSpec>,
        no_pipes: bool,
        controller_exception_filters: Vec<ScopedExceptionFilterSpec>,
        method_exception_filters: Vec<ScopedExceptionFilterSpec>,
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
        // Globals are run transport-level by the HTTP `HttpEndpointWrap`
        // that `use_guards_global` attaches; the per-route chain only
        // executes the controller/method scopes. Globals stay in the
        // chain *for dedup*: a controller/method declaration with the
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

impl Layer for RouteShaper {}

#[async_trait]
impl Interceptor for RouteShaper {
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

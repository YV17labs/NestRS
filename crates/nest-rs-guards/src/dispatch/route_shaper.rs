//! [`RouteShaper`] — the HTTP per-route shaper. A generic endpoint the
//! `#[routes]` macro wraps around each route at mount time. Orchestrates the
//! request-side layer families — guards and pipes — at the route scope,
//! deduplicating against the global chain by `TypeId`. The response-side
//! families (exception-filters, filters, interceptors) wrap the endpoint
//! *inside* the shaper — see [`route_layers`](crate::dispatch::route_layers).

use std::any::TypeId;

use nest_rs_core::layer_chain::{
    LayerSite, ResolvedLayer, compose_chain, dedup_bucket, resolve_global_layers,
};
use nest_rs_core::{Container, Layer};
use nest_rs_http::poem::{Body, Endpoint, Request, Response, Result};
use nest_rs_pipes::GlobalPipe;
use serde_json::Value;

use crate::Guard;
use crate::dispatch::denial_convert::deny_http;
use crate::dispatch::scoped_spec::{
    ScopedGuardSpec, ScopedPipeSpec, resolve_global_guards, resolve_specs,
};
use crate::registry::PipeSpecs;

/// HTTP per-route shaper: the deduped guard + pipe chains, wrapped around the
/// route's inner endpoint.
///
/// Built by [`wrap_route_shaper`] at mount time with the controller / method
/// scope specs. Resolves the global + per-route chain **eagerly against the
/// mount-time container** (the container is final at `configure`; resolving
/// lazily would only delay surfacing a broken chain to the first request),
/// dedups by `TypeId`, runs every layer in declaration order. No `#[public]`
/// skip — guards decide what `#[public]` means for them via the
/// [`Public`](nest_rs_http::Public) marker attached as request data.
///
/// Generic over the inner endpoint on purpose: the chains themselves stay
/// erased (`dyn Guard` / `dyn GlobalPipe` — composition is a mount-time,
/// runtime fact), but the wrap adds no per-request future boxing of its own.
pub struct RouteShaper<E> {
    guards: Vec<ResolvedLayer<dyn Guard>>,
    pipes: Vec<ResolvedLayer<dyn GlobalPipe>>,
    inner: E,
}

impl<E> Endpoint for RouteShaper<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Response> {
        for entry in &self.guards {
            // `as_ref()` dispatches straight on the erased guard: calling
            // through the `Guard for Arc<T>` blanket would nest a second
            // boxed future around every check, per guard, per request.
            if let Err(denial) = entry.layer.as_ref().check_http(&mut req).await {
                return Ok(deny_http(entry.name, denial));
            }
        }

        if !self.pipes.is_empty() {
            // Boxed on purpose: `apply_body_pipes` reads and rewrites the whole
            // JSON body, so one allocation is noise on that path — while
            // inlining its (large) state machine here would bloat the future of
            // every route that threads through [`ShapedRoute`], bare included,
            // and every such future is boxed per request by poem's route table.
            Box::pin(apply_body_pipes(&mut req, &self.pipes)).await?;
        }

        self.inner.call(req).await
    }
}

/// One HTTP route as `#[routes]` mounts it: shaped when the composed guard
/// **or** pipe chain is non-empty, bare (untouched) when both are empty.
///
/// The bare arm is not an access decision: with both chains empty the shaper's
/// loop bodies would be provable no-ops paid on every request. Fail-secure
/// posture is unchanged: the transport's unguarded-route scan warns at boot
/// independently, and the moment any guard or pipe reaches the route (global
/// pool, controller, method), the route mounts shaped.
pub enum ShapedRoute<E> {
    /// Both chains empty — the endpoint passes through untouched.
    Bare(E),
    /// At least one guard or pipe — the shaper runs before the inner endpoint.
    Shaped(RouteShaper<E>),
}

impl<E> Endpoint for ShapedRoute<E>
where
    E: Endpoint<Output = Response> + 'static,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        match self {
            Self::Bare(ep) => ep.call(req).await,
            Self::Shaped(ep) => ep.call(req).await,
        }
    }
}

/// Compose the route's guard / pipe chains and wrap `endpoint` in a
/// [`RouteShaper`] — or return it untouched when both chains are empty.
/// Emitted by `#[routes]` for every handler, mirroring the sibling
/// `wrap_route_*` helpers.
#[allow(clippy::too_many_arguments)]
pub fn wrap_route_shaper<E>(
    container: &Container,
    endpoint: E,
    route_label: &'static str,
    controller_guards: Vec<ScopedGuardSpec>,
    method_guards: Vec<ScopedGuardSpec>,
    force_guards: Vec<TypeId>,
    controller_pipes: Vec<ScopedPipeSpec>,
    method_pipes: Vec<ScopedPipeSpec>,
    no_pipes: bool,
) -> ShapedRoute<E>
where
    E: Endpoint<Output = Response> + 'static,
{
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
    if guards.is_empty() && pipes.is_empty() {
        return ShapedRoute::Bare(endpoint);
    }
    ShapedRoute::Shaped(RouteShaper {
        guards,
        pipes,
        inner: endpoint,
    })
}

fn resolve_guards(
    container: &Container,
    route_label: &str,
    controller_guards: &[ScopedGuardSpec],
    method_guards: &[ScopedGuardSpec],
    force_guards: &[TypeId],
) -> Vec<ResolvedLayer<dyn Guard>> {
    let global = resolve_global_guards(container);
    let controller = resolve_specs(container, controller_guards, LayerSite::Host);
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
    let global = resolve_global_layers::<PipeSpecs>(container);
    let controller = resolve_specs(container, controller_pipes, LayerSite::Host);
    let method = resolve_specs(container, method_pipes, LayerSite::Method);
    let chain =
        compose_chain::<dyn GlobalPipe>(dedup_bucket(global), controller, method, &[], route_label);
    log_effective_chain(route_label, "pipes", &chain);
    chain
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
    tracing::trace!(
        target: nest_rs_core::target::LAYERS,
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
    let limit = nest_rs_http::current_body_limit().unwrap_or(nest_rs_http::RawBody::DEFAULT_LIMIT);
    let body = req.take_body();
    let bytes = match body.into_bytes_limit(limit).await {
        Ok(b) => b,
        Err(nest_rs_http::poem::error::ReadBodyError::PayloadTooLarge) => {
            return Err(nest_rs_http::poem::Error::from_status(
                nest_rs_http::poem::http::StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        Err(err) => {
            // The body is already consumed and cannot be restored — continuing
            // would run the handler against an empty body with every global
            // pipe skipped. Fail the request instead, exactly as the sibling
            // body readers do (`nest_rs_http` `RawBody` / `Piped`).
            tracing::warn!(target: nest_rs_core::target::LAYERS, error = %err, "global pipe: failed to read body");
            return Err(err.into());
        }
    };
    if bytes.is_empty() {
        return Ok(());
    }
    let mut value: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(target: nest_rs_core::target::LAYERS, error = %err, "global pipe: body is not valid JSON");
            req.set_body(Body::from_bytes(bytes));
            return Ok(());
        }
    };
    for entry in pipes {
        if let Err(err) = entry.layer.transform_body(&mut value) {
            // One error format at the edge: a `400` RFC-9457
            // `application/problem+json` (`ProblemDetails`) — the pipe message
            // as `detail`, field-level errors as an `errors` extension member.
            let mut problem =
                nest_rs_http::ProblemDetails::bad_request().with_detail(err.message().to_owned());
            if let Some(details) = err.into_details() {
                problem = problem.with_extension("errors", details);
            }
            return Err(nest_rs_http::poem::Error::from(problem));
        }
    }
    // A re-serialization failure must not hand the handler an empty body
    // silently — fail the request instead.
    let rewritten = serde_json::to_vec(&value).map_err(|err| {
        tracing::error!(
            target: nest_rs_core::target::LAYERS,
            error = %err,
            "global pipe: failed to re-serialize the transformed body",
        );
        nest_rs_http::poem::Error::from_status(
            nest_rs_http::poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    req.set_body(Body::from_bytes(rewritten.into()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::{Layer, LayerSite};
    use nest_rs_http::poem::Body;
    use nest_rs_testing::LogCapture;

    use super::*;

    struct NoopPipe;

    impl Layer for NoopPipe {}
    impl GlobalPipe for NoopPipe {}

    fn pipe() -> Vec<ResolvedLayer<dyn GlobalPipe>> {
        vec![ResolvedLayer {
            type_id: std::any::TypeId::of::<NoopPipe>(),
            name: "NoopPipe",
            source: LayerSite::Global,
            layer: Arc::new(NoopPipe) as Arc<dyn GlobalPipe>,
        }]
    }

    /// A JSON request whose body stream dies mid-read — a client that hung up,
    /// or a proxy that closed the connection after the headers.
    fn request_with_a_failing_body() -> Request {
        let stream = futures_util::stream::once(async {
            Err::<Vec<u8>, _>(std::io::Error::other("the client hung up mid-body"))
        });
        Request::builder()
            .method(nest_rs_http::poem::http::Method::POST)
            .uri("/things".parse().expect("a uri"))
            .content_type("application/json")
            .body(Body::from_bytes_stream(stream))
    }

    #[tokio::test]
    async fn a_body_the_global_pipes_could_not_read_fails_the_request_and_says_why() {
        // The reason this cannot degrade quietly: reading the body *consumes*
        // it, and a partial read cannot be put back. Carrying on would run the
        // handler against an empty body with every global pipe skipped — a
        // request that looks served, with the app's edge validation silently
        // absent from it. So the request fails, and this line is what says the
        // failure was the body rather than the handler.
        let logs = LogCapture::install();
        let mut req = request_with_a_failing_body();

        let err = apply_body_pipes(&mut req, &pipe())
            .await
            .expect_err("a body that cannot be read is not a body the pipes ran on");
        assert_eq!(
            err.status(),
            nest_rs_http::poem::http::StatusCode::BAD_REQUEST,
        );

        let event = logs.expect_one("nest_rs::layers", "global pipe: failed to read body");
        assert_eq!(event.level, "warn");
        assert!(
            event.field("error").is_some_and(|e| e.contains("hung up")),
            "the event carries the read failure, got {:?}",
            event.fields,
        );
    }

    #[tokio::test]
    async fn an_oversized_body_is_the_status_the_limit_names_and_not_this_line() {
        // Its neighbour, and the reason the branch is split: `PayloadTooLarge`
        // is the body limit doing its job, so it answers `413` and says nothing
        // — folding the two would file every oversized upload under a read
        // failure.
        let logs = LogCapture::install();
        let mut req = Request::builder()
            .method(nest_rs_http::poem::http::Method::POST)
            .uri("/things".parse().expect("a uri"))
            .content_type("application/json")
            .body(Body::from_bytes(vec![b'x'; 64 * 1024 * 1024].into()));

        let err = apply_body_pipes(&mut req, &pipe())
            .await
            .expect_err("a body past the limit is refused");
        assert_eq!(
            err.status(),
            nest_rs_http::poem::http::StatusCode::PAYLOAD_TOO_LARGE,
        );
        logs.expect_none("nest_rs::layers", "global pipe: failed to read body");
    }
}

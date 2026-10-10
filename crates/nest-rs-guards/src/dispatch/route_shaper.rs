//! [`RouteShaper`] — the HTTP per-route shaper. A generic endpoint the
//! `#[routes]` macro wraps around each route at mount time. Orchestrates the
//! request-side layer families — guards and pipes — at the route scope,
//! deduplicating against the global chain by `TypeId`. The response-side
//! families (exception-filters, filters, interceptors) wrap the endpoint
//! *inside* the shaper — see [`route_layers`](crate::dispatch::route_layers).

use std::any::TypeId;

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, refuse_site, resolve_global_layers,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, Layer};
use nest_rs_http::poem::{Body, Endpoint, Request, Response, Result};
use nest_rs_pipes::GlobalPipe;
use serde_json::Value;

use crate::dispatch::denial_convert::{denial_to_http_response, deny_http};
use crate::dispatch::scoped_spec::{
    ScopedGuardSpec, ScopedPipeSpec, report_unresolved, resolve_global_guards, resolve_scoped,
};
use crate::dispatch::validate::validate_guard_chain;
use crate::registry::PipeSpecs;
use crate::{Denial, Guard};

/// HTTP per-route shaper: the deduped guard + pipe chains, wrapped around the
/// route's inner endpoint.
///
/// Built by [`wrap_route_shaper`] at mount, resolving the global + per-route
/// chain eagerly so a broken chain surfaces at boot; dedups by `TypeId` and
/// runs every layer in declaration order. No `#[public]` skip: guards read the
/// [`Public`](nest_rs_http::Public) marker themselves.
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
            // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
            // blanket would nest a second boxed future per check.
            if let Err(denial) = entry.layer.as_ref().check_http(&mut req).await {
                return Ok(deny_http(entry.name, denial));
            }
        }

        if !self.pipes.is_empty() {
            // Boxed on purpose: inlining `apply_body_pipes`' large state machine
            // would bloat every route's future, which poem boxes per request.
            Box::pin(apply_body_pipes(&mut req, &self.pipes)).await?;
        }

        self.inner.call(req).await
    }
}

/// One HTTP route as `#[routes]` mounts it: shaped when the composed guard
/// **or** pipe chain is non-empty, bare (untouched) when both are empty, and
/// refused when its chains did not compose.
///
/// The bare arm is not an access decision: the boot's unguarded-route scan
/// warns independently.
pub enum ShapedRoute<E> {
    /// Both chains empty — the endpoint passes through untouched.
    Bare(E),
    /// At least one guard or pipe — the shaper runs before the inner endpoint.
    Shaped(RouteShaper<E>),
    /// The route's chains did not compose: the boot fails on the refusal it
    /// filed, and a server that never read it answers an opaque `500`.
    Refused,
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
            Self::Refused => Ok(refused_route()),
        }
    }
}

/// What a route whose chains did not compose answers: an opaque `500`, never
/// the handler without the layers it declares.
pub(crate) fn refused_route() -> Response {
    denial_to_http_response(Denial::internal("the route's layer chain did not compose"))
}

/// Compose the route's guard / pipe chains and wrap `endpoint` in a
/// [`RouteShaper`] — or return it untouched when both chains are empty.
/// Emitted by `#[routes]` for every handler.
///
/// A declared layer that does not resolve, or a method scope that orders an
/// authorization guard before the authentication guard whose principal it
/// reads, refuses the boot through the container: a mount has no `Result`, so
/// the route answers an opaque `500` in the meantime.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one chain a decorator collected; a struct would only rename them"
)]
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
    let composed = resolve_guards(
        container,
        route_label,
        &controller_guards,
        &method_guards,
        &force_guards,
    )
    .and_then(|guards| {
        let pipes = if no_pipes {
            // `#[no_pipes]` skips every pipe — globals, controller, method.
            Vec::new()
        } else {
            resolve_pipes(container, route_label, &controller_pipes, &method_pipes)?
        };
        Ok((guards, pipes))
    });
    let (guards, pipes) = match composed {
        Ok(chains) => chains,
        Err(refusal) => {
            refuse_site(container, refusal);
            return ShapedRoute::Refused;
        }
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
) -> nest_rs_core::anyhow::Result<Vec<ResolvedLayer<dyn Guard>>> {
    let global = resolve_global_guards(container);
    let controller = resolve_scoped(container, controller_guards, LayerSite::Host, route_label)
        .inspect_err(report_unresolved)?;
    let method = resolve_scoped(container, method_guards, LayerSite::Method, route_label)
        .inspect_err(report_unresolved)?;
    let chain = compose_chain::<dyn Guard>(
        dedup_bucket(global),
        controller,
        method,
        force_guards,
        route_label,
    );
    // The controller's boot check validated the global and controller scopes; a
    // method scope makes this route's chain one of its own.
    if !method_guards.is_empty() || !force_guards.is_empty() {
        validate_guard_chain(route_label, &chain).map_err(nest_rs_core::anyhow::Error::msg)?;
    }
    log_effective_chain(route_label, "guards", &chain);
    // The single execution site for the guard pool on a routed handler, after
    // routing so a guard reads `#[public]`.
    Ok(chain)
}

fn resolve_pipes(
    container: &Container,
    route_label: &str,
    controller_pipes: &[ScopedPipeSpec],
    method_pipes: &[ScopedPipeSpec],
) -> nest_rs_core::anyhow::Result<Vec<ResolvedLayer<dyn GlobalPipe>>> {
    let global = resolve_global_layers::<PipeSpecs>(container);
    let controller = resolve_scoped(container, controller_pipes, LayerSite::Host, route_label)
        .inspect_err(report_unresolved)?;
    let method = resolve_scoped(container, method_pipes, LayerSite::Method, route_label)
        .inspect_err(report_unresolved)?;
    let chain =
        compose_chain::<dyn GlobalPipe>(dedup_bucket(global), controller, method, &[], route_label);
    log_effective_chain(route_label, "pipes", &chain);
    Ok(chain)
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
            // The body is consumed and cannot be restored: fail rather than run
            // the handler with every global pipe skipped.
            tracing::warn!(target: nest_rs_core::target::LAYERS, error = %nest_rs_core::error_message(&err), "global pipe: failed to read body");
            return Err(err.into());
        }
    };
    if bytes.is_empty() {
        return Ok(());
    }
    let mut value: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(target: nest_rs_core::target::LAYERS, error = %nest_rs_core::error_message(&err), "global pipe: body is not valid JSON");
            req.set_body(Body::from_bytes(bytes));
            return Ok(());
        }
    };
    for entry in pipes {
        if let Err(err) = entry.layer.transform_body(&mut value) {
            // A `400` RFC 9457 problem: the pipe message as `detail`, field-level
            // errors as an `errors` extension member.
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
            error = %nest_rs_core::error_message(&err),
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

    use nest_rs_core::__private::take_site_refusal;
    use nest_rs_core::{Layer, LayerSite, LayerSpec};
    use nest_rs_http::poem::Body;
    use nest_rs_http::poem::http::StatusCode;
    use nest_rs_testing::LogCapture;

    use super::*;

    /// A layer no imported module provides: its spec resolves to nothing.
    struct Unprovided;

    fn unprovided<L: ?Sized>() -> LayerSpec<L> {
        LayerSpec::new(TypeId::of::<Unprovided>(), "GhostLayer", |_| None)
    }

    /// The request-side slots `#[routes]` fills for one route.
    #[derive(Clone, Copy, Debug)]
    enum Slot {
        ControllerGuard,
        MethodGuard,
        ControllerPipe,
        MethodPipe,
    }

    #[tokio::test]
    async fn a_route_layer_no_module_provides_refuses_the_boot_and_answers_500_meanwhile() {
        for slot in [
            Slot::ControllerGuard,
            Slot::MethodGuard,
            Slot::ControllerPipe,
            Slot::MethodPipe,
        ] {
            let logs = LogCapture::install();
            let container = Container::default();
            let mut controller_guards: Vec<ScopedGuardSpec> = Vec::new();
            let mut method_guards: Vec<ScopedGuardSpec> = Vec::new();
            let mut controller_pipes: Vec<ScopedPipeSpec> = Vec::new();
            let mut method_pipes: Vec<ScopedPipeSpec> = Vec::new();
            match slot {
                Slot::ControllerGuard => controller_guards.push(unprovided()),
                Slot::MethodGuard => method_guards.push(unprovided()),
                Slot::ControllerPipe => controller_pipes.push(unprovided()),
                Slot::MethodPipe => method_pipes.push(unprovided()),
            }

            let route = wrap_route_shaper(
                &container,
                nest_rs_http::poem::endpoint::make_sync(|_| {
                    Response::builder().body("the handler ran")
                }),
                "get /x",
                controller_guards,
                method_guards,
                Vec::new(),
                controller_pipes,
                method_pipes,
                false,
            );
            assert!(
                matches!(route, ShapedRoute::Refused),
                "{slot:?}: the route would run without the layer it declares",
            );
            let response = route
                .call(Request::default())
                .await
                .expect("a refused route answers");
            assert_eq!(
                response.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{slot:?}"
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
        // A partial read cannot be put back, so the request fails; this line
        // says the failure was the body rather than the handler.
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
        // `PayloadTooLarge` is the body limit doing its job: `413`, and no
        // read-failure line.
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

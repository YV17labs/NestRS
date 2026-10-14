//! [`RouteShaper`] — the HTTP per-route shaper, a generic endpoint the
//! `#[routes]` macro wraps around each route at mount time. It runs the
//! request-side layer families at the route scope, deduplicated against the
//! global chain by `TypeId`: the guard stage, the route's class gates, then the
//! pipes. The response-side families (exception-filters, filters,
//! interceptors) wrap the endpoint *inside* it.

use std::any::TypeId;

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, log_effective_chain, refuse_site,
    report_unresolved, resolve_global_layers, resolve_scoped,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, Handler};
use nest_rs_guards::__private::{resolve_global_guards, validate_guard_chain};
use nest_rs_guards::{Denial, Guard, GuardSpec};
use nest_rs_pipes::{GlobalPipe, PipeSpec, PipeSpecs};
use poem::error::ReadBodyError;
use poem::{Body, Endpoint, Request, Response, Result};
use serde_json::Value;

use crate::error::BodyError;
use crate::guard_stage::denial_to_http_response;
use crate::shaper::GateFn;

/// HTTP per-route shaper: the deduped guard stage, the route's class gates and
/// the deduped pipe chain, wrapped around the route's inner endpoint, which
/// runs under what the guards attached.
///
/// Built by [`wrap_route_shaper`] at mount, resolving the global + per-route
/// chain eagerly so a broken chain surfaces at boot; dedups by `TypeId` and
/// runs every layer in declaration order. No `#[public]` skip: guards read the
/// route's posture themselves, off its `Handler`.
pub struct RouteShaper<E> {
    handler: &'static Handler,
    guards: Vec<ResolvedLayer<dyn Guard>>,
    gates: Vec<GateFn>,
    pipes: Vec<ResolvedLayer<dyn GlobalPipe>>,
    masked: bool,
    inner: E,
}

impl<E> Endpoint for RouteShaper<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        // What the guards attach is withheld from the reads a mask would scope
        // when no response shaper masks the route's reply.
        crate::guard_stage::guard_then(
            self.handler,
            &self.guards,
            req,
            |req| self.after_the_guards(req),
            !self.masked,
        )
        .await
    }
}

impl<E> RouteShaper<E>
where
    E: Endpoint<Output = Response>,
{
    /// Everything after the guard stage, under what it attached.
    async fn after_the_guards(&self, mut req: Request) -> Result<Response> {
        // After the guards, whose ability it reads; before the pipes, so a
        // refused caller pays for no validation; and outside every route-scoped
        // interceptor and filter, so none answers or maps what it refuses.
        for gate in &self.gates {
            if let Some(refusal) = gate(&req) {
                return Ok(refusal);
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

/// One HTTP route as `#[routes]` mounts it: shaped when any request-side layer
/// runs at the route — a guard, a class gate, a pipe — bare (untouched) when
/// none does, and refused when its chains did not compose.
///
/// The bare arm is not an access decision: the boot's unguarded-route scan
/// warns independently.
pub enum ShapedRoute<E> {
    /// No layer at the route — the endpoint passes through untouched.
    Bare(E),
    /// At least one layer — the shaper runs before the inner endpoint.
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

/// What a site whose chains did not compose answers: an opaque `500`, never
/// the handler without the layers it declares.
pub fn refused_route() -> Response {
    denial_to_http_response(Denial::internal("the route's layer chain did not compose"))
}

/// Compose the route's guard / pipe chains and wrap `endpoint` in a
/// [`RouteShaper`] running them around the route's class `gates`, as the unit
/// `handler` declares — or return it untouched when no layer runs at the route.
/// Emitted by `#[routes]` for every handler, `gates` selected by type from what
/// the route declares, `masked` whether a response shaper masks its reply.
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
    controller_guards: Vec<GuardSpec>,
    method_guards: Vec<GuardSpec>,
    force_guards: Vec<TypeId>,
    controller_pipes: Vec<PipeSpec>,
    method_pipes: Vec<PipeSpec>,
    no_pipes: bool,
    gates: Vec<GateFn>,
    masked: bool,
    handler: &'static Handler,
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
    if guards.is_empty() && gates.is_empty() && pipes.is_empty() {
        return ShapedRoute::Bare(endpoint);
    }
    ShapedRoute::Shaped(RouteShaper {
        handler,
        guards,
        gates,
        pipes,
        masked,
        inner: endpoint,
    })
}

fn resolve_guards(
    container: &Container,
    route_label: &str,
    controller_guards: &[GuardSpec],
    method_guards: &[GuardSpec],
    force_guards: &[TypeId],
) -> anyhow::Result<Vec<ResolvedLayer<dyn Guard>>> {
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
        validate_guard_chain(route_label, &chain).map_err(anyhow::Error::msg)?;
    }
    log_effective_chain(route_label, "guards", &chain);
    // The single execution site for the guard pool on a routed handler, after
    // routing so a guard reads `#[public]`.
    Ok(chain)
}

fn resolve_pipes(
    container: &Container,
    route_label: &str,
    controller_pipes: &[PipeSpec],
    method_pipes: &[PipeSpec],
) -> anyhow::Result<Vec<ResolvedLayer<dyn GlobalPipe>>> {
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

/// Whether the transport's body wrapper ended this read — past the cap, or no
/// frame within its gap. The client did, and the edge answers it, `413` or
/// `408`, whatever this read returns.
fn stopped_by_the_edge(err: &ReadBodyError) -> bool {
    let ReadBodyError::Io(err) = err else {
        return false;
    };
    matches!(
        err.get_ref()
            .and_then(|inner| inner.downcast_ref::<BodyError>()),
        Some(BodyError::TooLarge { .. } | BodyError::Stalled { .. })
    )
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
        .get(poem::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !content_type.contains("json") {
        return Ok(());
    }
    let limit = crate::current_body_limit();
    let body = req.take_body();
    let bytes = match body.into_bytes_limit(limit).await {
        Ok(b) => b,
        Err(ReadBodyError::PayloadTooLarge) => {
            return Err(poem::Error::from_status(
                poem::http::StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        Err(err) => {
            // The body is consumed and cannot be restored: fail rather than run
            // the handler with every global pipe skipped.
            if !stopped_by_the_edge(&err) {
                tracing::warn!(target: nest_rs_core::target::LAYERS, error = %nest_rs_core::error_message(&err), "global pipe: failed to read body");
            }
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
                crate::ProblemDetails::bad_request().with_detail(err.message().to_owned());
            if let Some(details) = err.into_details() {
                problem = problem.with_extension("errors", details);
            }
            return Err(poem::Error::from(problem));
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
        poem::Error::from_status(poem::http::StatusCode::INTERNAL_SERVER_ERROR)
    })?;
    req.set_body(Body::from_bytes(rewritten.into()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::__private::take_site_refusal;
    use nest_rs_core::{Layer, LayerSite, LayerSpec};
    use nest_rs_testing::LogCapture;
    use poem::Body;
    use poem::http::StatusCode;

    use super::*;

    /// A layer no imported module provides: its spec resolves to nothing.
    struct Unprovided;

    struct Routes;

    static ROUTE: Handler =
        nest_rs_core::__private::declare_handler(nest_rs_core::__private::HandlerDeclaration {
            unit: crate::unit::REQUEST,
            host: TypeId::of::<Routes>(),
            host_name: "Routes",
            method: "route",
            address: "/x",
            posture: nest_rs_core::Posture::Gated,
            meta: nest_rs_core::__private::MetaLevels::default,
        });

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
            let mut controller_guards: Vec<GuardSpec> = Vec::new();
            let mut method_guards: Vec<GuardSpec> = Vec::new();
            let mut controller_pipes: Vec<PipeSpec> = Vec::new();
            let mut method_pipes: Vec<PipeSpec> = Vec::new();
            match slot {
                Slot::ControllerGuard => controller_guards.push(unprovided()),
                Slot::MethodGuard => method_guards.push(unprovided()),
                Slot::ControllerPipe => controller_pipes.push(unprovided()),
                Slot::MethodPipe => method_pipes.push(unprovided()),
            }

            let route = wrap_route_shaper(
                &container,
                poem::endpoint::make_sync(|_| Response::builder().body("the handler ran")),
                "get /x",
                controller_guards,
                method_guards,
                Vec::new(),
                controller_pipes,
                method_pipes,
                false,
                Vec::new(),
                true,
                &ROUTE,
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
            .method(poem::http::Method::POST)
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
        assert_eq!(err.status(), StatusCode::BAD_REQUEST,);

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
            .method(poem::http::Method::POST)
            .uri("/things".parse().expect("a uri"))
            .content_type("application/json")
            .body(Body::from_bytes(vec![b'x'; 64 * 1024 * 1024].into()));

        let err = apply_body_pipes(&mut req, &pipe())
            .await
            .expect_err("a body past the limit is refused");
        assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE,);
        logs.expect_none("nest_rs::layers", "global pipe: failed to read body");
    }

    /// The endpoint inside holds the route's own interceptors and filters, so a
    /// gate answering before it answers before them.
    #[tokio::test]
    async fn a_route_gate_alone_shapes_the_route_and_answers_before_the_endpoint() {
        fn refuse_unless_admitted(req: &Request) -> Option<Response> {
            (!req.headers().contains_key("x-admitted"))
                .then(|| Response::builder().status(StatusCode::FORBIDDEN).finish())
        }

        let container = Container::default();
        let route = wrap_route_shaper(
            &container,
            poem::endpoint::make_sync(|_| Response::builder().body("the handler ran")),
            "get /gated",
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
            vec![refuse_unless_admitted as GateFn],
            true,
            &ROUTE,
        );
        assert!(
            matches!(route, ShapedRoute::Shaped(_)),
            "a gate alone shapes the route"
        );

        let refused = route.call(Request::default()).await.expect("answers");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        let admitted = route
            .call(Request::builder().header("x-admitted", "").finish())
            .await
            .expect("answers");
        assert_eq!(admitted.status(), StatusCode::OK);
    }
}

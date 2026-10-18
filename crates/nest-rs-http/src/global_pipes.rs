//! The global pipes over an HTTP request's body: the chain a route composes
//! at mount from the app's pool and its own `#[use_pipes]`, run after its
//! guards, before its handler reads the body.

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, log_effective_chain, report_unresolved,
    resolve_global_layers, resolve_scoped,
};
use nest_rs_core::Container;
use nest_rs_core::Handler;
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_pipes::{ArgumentKind, GlobalPipe, PipeSpec, PipeSpecs};
use serde_json::Value;

use crate::body::Body;
use crate::error::{BodyError, HttpError, Result};
use crate::request::Request;
use crate::view::with_view;

/// The global pipes one route runs over its request's body, composed once at
/// mount: the app's pool, then the host's, then the method's, each once.
pub(crate) struct GlobalPipes(Vec<ResolvedLayer<dyn GlobalPipe>>);

impl GlobalPipes {
    /// No pipe: `#[no_pipes]`, or nothing declared anywhere.
    pub(crate) fn none() -> Self {
        Self(Vec::new())
    }

    /// The chain `route_label` composes on `container`: the pool, then the
    /// host's and the method's specs.
    pub(crate) fn resolve(
        container: &Container,
        route_label: &str,
        host: &[PipeSpec],
        method: &[PipeSpec],
    ) -> anyhow::Result<Self> {
        resolve_pipes(container, route_label, host, method).map(Self)
    }

    /// Whether the chain runs nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Hand `req`'s JSON body to every pipe, as the unit `handler` declares.
    pub(crate) async fn apply(&self, handler: &'static Handler, req: &mut Request) -> Result<()> {
        apply_body_pipes(handler, req, &self.0).await
    }
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
fn stopped_by_the_edge(err: &BodyError) -> bool {
    let BodyError::Io(err) = err else {
        return false;
    };
    matches!(
        err.get_ref()
            .and_then(|inner| inner.downcast_ref::<BodyError>()),
        Some(BodyError::TooLarge { .. } | BodyError::Stalled { .. })
    )
}

/// Read the JSON body, hand it to every pipe in order as the route's unit,
/// write the rewritten body back into the request. No-op when the body is
/// missing / not JSON / no pipe rejects.
async fn apply_body_pipes(
    handler: &'static Handler,
    req: &mut Request,
    pipes: &[ResolvedLayer<dyn GlobalPipe>],
) -> Result<()> {
    if !req.content_type().is_some_and(|ct| ct.contains("json")) {
        return Ok(());
    }
    let limit = crate::current_body_limit();
    let bytes = match req.take_body().into_bytes_limit(limit).await {
        Ok(bytes) => bytes,
        Err(err @ BodyError::TooLarge { .. }) => return Err(err.into()),
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
    // The body is out of the request while the pipes read it, so the view holds
    // the head alone.
    let folded = with_view(handler, req, |cx| {
        pipes
            .iter()
            .try_for_each(|entry| entry.layer.transform(cx, ArgumentKind::Body, &mut value))
    });
    if let Err(err) = folded {
        // A `400` RFC 9457 problem: the pipe message as `detail`, field-level
        // errors as an `errors` extension member.
        let mut problem =
            crate::ProblemDetails::bad_request().with_detail(err.message().to_owned());
        if let Some(details) = err.into_details() {
            problem = problem.with_extension("errors", details);
        }
        return Err(HttpError::from(problem));
    }
    // A re-serialization failure must not hand the handler an empty body
    // silently — fail the request instead.
    let rewritten = serde_json::to_vec(&value).map_err(|err| {
        tracing::error!(
            target: nest_rs_core::target::LAYERS,
            error = %nest_rs_core::error_message(&err),
            "global pipe: failed to re-serialize the transformed body",
        );
        HttpError::from_status(http::StatusCode::INTERNAL_SERVER_ERROR)
    })?;
    req.set_body(Body::from_bytes(rewritten.into()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::sync::Arc;

    use http::StatusCode;
    use nest_rs_core::Layer;
    use nest_rs_testing::LogCapture;

    use super::*;
    use crate::{HeaderValue, Method, Uri, header};

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

    fn json_post(body: impl Into<Body>) -> Request {
        Request::builder()
            .method(Method::POST)
            .uri(Uri::from_static("/things"))
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )
            .body(body)
    }

    /// A JSON request whose body stream dies mid-read — a client that hung up,
    /// or a proxy that closed the connection after the headers.
    fn request_with_a_failing_body() -> Request {
        let stream = futures_util::stream::once(async {
            Err::<bytes::Bytes, _>(std::io::Error::other("the client hung up mid-body"))
        });
        json_post(Body::from_stream(stream))
    }

    #[tokio::test]
    async fn a_body_the_global_pipes_could_not_read_fails_the_request_and_says_why() {
        // A partial read cannot be put back, so the request fails; this line
        // says the failure was the body rather than the handler.
        let logs = LogCapture::install();
        let mut req = request_with_a_failing_body();

        let err = apply_body_pipes(&ROUTE, &mut req, &pipe())
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
        let mut req = json_post(vec![b'x'; 64 * 1024 * 1024]);

        let err = apply_body_pipes(&ROUTE, &mut req, &pipe())
            .await
            .expect_err("a body past the limit is refused");
        assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE,);
        logs.expect_none("nest_rs::layers", "global pipe: failed to read body");
    }
}

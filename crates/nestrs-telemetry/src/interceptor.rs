use async_trait::async_trait;
use nestrs_config::env_var;
use nestrs_http::interceptor;
use nestrs_middleware::{Interceptor, Next};
use poem::{Request, Response, Result};

#[cfg(feature = "otlp")]
use {
    bytes::Bytes,
    futures_core::Stream,
    opentelemetry::global,
    opentelemetry::trace::TraceContextExt,
    opentelemetry_http::HeaderExtractor,
    poem::http::{HeaderName, HeaderValue},
    poem::Body,
    std::io::Error as IoError,
    std::pin::Pin,
    std::task::{Context, Poll},
    std::time::Instant,
    tracing::Instrument,
    tracing_opentelemetry::OpenTelemetrySpanExt,
};

/// Per-request HTTP observation.
///
/// Opens a `tracing` span with OTel HTTP semantic-convention attributes,
/// parents it on an incoming W3C `traceparent`, surfaces the trace id as
/// `X-Trace-Id`. The span is exported but **not** rendered in the console
/// (`FmtSpan::NONE`).
///
/// One visible access event per request (`tracing::info!` on target
/// `nestrs::access`) emitted at **end-of-body**, so `bytes` and `duration_ms`
/// are exact — poem stamps `Content-Length` past this interceptor, so the
/// body is wrapped in a byte-counting stream. A client disconnect still emits
/// via `Drop`.
///
/// Toggle via `NESTRS_HTTP__ACCESS_LOG` (default `true`); the OTel span is
/// always created so propagation and OTLP export keep working.
#[interceptor]
#[derive(Clone, Copy, Debug)]
pub(crate) struct OtelHttp {
    access_log: bool,
}

impl Default for OtelHttp {
    fn default() -> Self {
        let access_log = env_var("NESTRS_HTTP__ACCESS_LOG")
            .map(|raw| {
                !matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(true);
        Self { access_log }
    }
}

#[cfg(feature = "otlp")]
const X_TRACE_ID: HeaderName = HeaderName::from_static("x-trace-id");

#[async_trait]
impl Interceptor for OtelHttp {
    #[allow(unused_mut, unused_variables)]
    async fn intercept(&self, mut req: Request, next: Next<'_>) -> Result<Response> {
        #[cfg(feature = "otlp")]
        {
            let method = req.method().clone();
            let path = req.uri().path().to_string();
            let client_ip = client_ip(&req);
            let user_agent = user_agent(&req).unwrap_or_default();

            let span = tracing::info_span!(
                "http.request",
                otel.kind = "server",
                http.request.method = %method,
                http.route = %path,
                client.address = %client_ip,
                user_agent.original = %user_agent,
                http.response.status_code = tracing::field::Empty,
                http.response.body.size = tracing::field::Empty,
            );

            // RwLock + alloc; skip for the common no-traceparent case.
            if req.headers().contains_key("traceparent") {
                let parent_cx = global::get_text_map_propagator(|prop| {
                    prop.extract(&HeaderExtractor(req.headers()))
                });
                let _ = span.set_parent(parent_cx);
            }

            let trace_id = current_trace_id(&span).unwrap_or_default();
            let trace_header = HeaderValue::from_str(&trace_id).ok();

            let start = Instant::now();
            let result = next.run(req).instrument(span.clone()).await;

            // Normalise to a Response so an error response is measured too.
            // OtelHttp is the outermost discovered interceptor, so swallowing
            // the Err into its rendered response is invisible to outer layers.
            let mut response = result.unwrap_or_else(|err| err.into_response());
            let status = response.status().as_u16();
            span.record("http.response.status_code", status);
            if let Some(val) = trace_header {
                response.headers_mut().insert(X_TRACE_ID, val);
            }

            // Body wrapper fires the access event at end-of-body with exact
            // bytes/duration; the span clone keeps the OTel span open until
            // body.size is recorded.
            let (parts, body) = response.into_parts();
            let logged = AccessLogBody {
                inner: Box::pin(body.into_bytes_stream()),
                counted: 0,
                log: Some(AccessLog {
                    method,
                    path,
                    status,
                    client_ip,
                    user_agent,
                    trace_id,
                    start,
                    span,
                    access_log: self.access_log,
                }),
            };
            Ok(Response::from_parts(parts, Body::from_bytes_stream(logged)))
        }
        #[cfg(not(feature = "otlp"))]
        {
            next.run(req).await
        }
    }
}

#[cfg(feature = "otlp")]
struct AccessLog {
    method: poem::http::Method,
    path: String,
    status: u16,
    client_ip: String,
    user_agent: String,
    trace_id: String,
    start: Instant,
    span: tracing::Span,
    access_log: bool,
}

#[cfg(feature = "otlp")]
impl AccessLog {
    fn emit(self, bytes: u64) {
        self.span.record("http.response.body.size", bytes);
        if self.access_log {
            let duration_ms = self.start.elapsed().as_secs_f64() * 1e3;
            tracing::info!(
                target: "nestrs::access",
                method = %self.method,
                path = %self.path,
                status = self.status,
                bytes = bytes,
                duration_ms = duration_ms,
                client_ip = %self.client_ip,
                user_agent = %self.user_agent,
                trace_id = %self.trace_id,
            );
        }
    }
}

#[cfg(feature = "otlp")]
struct AccessLogBody {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, IoError>> + Send>>,
    counted: u64,
    log: Option<AccessLog>,
}

#[cfg(feature = "otlp")]
impl AccessLogBody {
    /// At most once — covers both stream-end and drop-on-disconnect.
    fn emit_once(&mut self) {
        if let Some(log) = self.log.take() {
            log.emit(self.counted);
        }
    }
}

#[cfg(feature = "otlp")]
impl Stream for AccessLogBody {
    type Item = Result<Bytes, IoError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.counted += chunk.len() as u64;
                Poll::Ready(Some(Ok(chunk)))
            }
            terminal @ Poll::Ready(_) => {
                this.emit_once();
                terminal
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(feature = "otlp")]
impl Drop for AccessLogBody {
    fn drop(&mut self) {
        self.emit_once();
    }
}

#[cfg(feature = "otlp")]
fn current_trace_id(span: &tracing::Span) -> Option<String> {
    let otel_ctx = span.context();
    let span_ctx = otel_ctx.span().span_context().clone();
    span_ctx.is_valid().then(|| span_ctx.trace_id().to_string())
}

#[cfg(feature = "otlp")]
fn client_ip(req: &Request) -> String {
    // Strip port for readability; fall back to Display for non-TCP (UDS tests).
    req.remote_addr()
        .as_socket_addr()
        .map(|sa| sa.ip().to_string())
        .unwrap_or_else(|| req.remote_addr().to_string())
}

#[cfg(feature = "otlp")]
fn user_agent(req: &Request) -> Option<String> {
    req.headers()
        .get(poem::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

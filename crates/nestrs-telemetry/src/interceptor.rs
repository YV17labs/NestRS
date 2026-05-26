use async_trait::async_trait;
use nestrs_core::config::env_var;
use nestrs_http::interceptor;
use nestrs_middleware::{Interceptor, Next};
use poem::{Request, Response, Result};

#[cfg(feature = "otlp")]
use {
    opentelemetry::global,
    opentelemetry::trace::TraceContextExt,
    opentelemetry_http::HeaderExtractor,
    poem::http::{HeaderName, HeaderValue},
    std::time::Instant,
    tracing::Instrument,
    tracing_opentelemetry::OpenTelemetrySpanExt,
};

/// Per-request HTTP observation.
///
/// Opens a `tracing` span with OTel HTTP semantic-convention attributes
/// (`http.request.method`, `http.route`, `http.response.status_code`,
/// `otel.kind`), parents it on any incoming W3C `traceparent` header, and
/// surfaces the trace id as an `X-Trace-Id` response header. The span is
/// what OTel sees and exports; **it is not rendered** in the console
/// (`FmtSpan::NONE`).
///
/// The visible per-request log line is a single `tracing::info!` event
/// (target `nestrs::access`) emitted at request end, with short, structured
/// field names (`method`, `path`, `status`, `duration_ms`, `trace_id`).
/// One event = one line in text mode, one JSON object in JSON mode — no
/// span-context prefix, no duplication.
///
/// Toggle the access event via the `NESTRS_HTTP__ACCESS_LOG` env var
/// (default `true`; falsy values `0`/`false`/`off`/`no` disable). The OTel
/// span is always created so `traceparent` propagation and OTLP export keep
/// working.
///
/// Crate-private: registered by [`crate::TelemetryModule`], so an app activates
/// it with `imports = [TelemetryModule]` and never names this type.
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
            let user_agent = user_agent(&req);
            let ua = user_agent.as_deref().unwrap_or("");

            let span = tracing::info_span!(
                "http.request",
                otel.kind = "server",
                http.request.method = %method,
                http.route = %path,
                client.address = %client_ip,
                user_agent.original = %ua,
                http.response.status_code = tracing::field::Empty,
                http.response.body.size = tracing::field::Empty,
            );

            // The propagator lookup + HeaderExtractor walk costs a RwLock read
            // and an allocation; skip it for the common no-traceparent case.
            if req.headers().contains_key("traceparent") {
                let parent_cx = global::get_text_map_propagator(|prop| {
                    prop.extract(&HeaderExtractor(req.headers()))
                });
                let _ = span.set_parent(parent_cx);
            }

            let trace_id = current_trace_id(&span);
            let trace_header = trace_id
                .as_deref()
                .and_then(|tid| HeaderValue::from_str(tid).ok());

            let start = Instant::now();
            let res = next.run(req).instrument(span.clone()).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            let (status, bytes, out) = match res {
                Ok(mut r) => {
                    let s = r.status().as_u16();
                    let b = response_bytes(&r);
                    span.record("http.response.status_code", s);
                    span.record("http.response.body.size", b);
                    if let Some(val) = trace_header {
                        r.headers_mut().insert(X_TRACE_ID, val);
                    }
                    (s, b, Ok(r))
                }
                Err(err) => {
                    let s = err.status().as_u16();
                    span.record("http.response.status_code", s);
                    (s, 0, Err(err))
                }
            };

            // Emit *outside* the span scope so the console line has no span
            // context prefix — just one clean line per request.
            if self.access_log {
                tracing::info!(
                    target: "nestrs::access",
                    method = %method,
                    path = %path,
                    status = status,
                    bytes = bytes,
                    duration_ms = duration_ms,
                    client_ip = %client_ip,
                    user_agent = %ua,
                    trace_id = trace_id.as_deref().unwrap_or(""),
                );
            }

            out
        }
        #[cfg(not(feature = "otlp"))]
        {
            next.run(req).await
        }
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
    // Strip the port for log readability; fall back to the raw Display when
    // the connection isn't a TCP socket (e.g. UDS in tests).
    req.remote_addr()
        .as_socket_addr()
        .map(|sa| sa.ip().to_string())
        .unwrap_or_else(|| req.remote_addr().to_string())
}

#[cfg(feature = "otlp")]
fn response_bytes(res: &Response) -> u64 {
    // Poem hides the body (`BoxBody` is `pub(crate)`) and stamps Content-Length
    // at wire time, past this point — so streamed/chunked responses, and any the
    // handler didn't set CL on, log `0`, matching Apache's `%B` for unknown sizes.
    res.headers()
        .get(poem::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(feature = "otlp")]
fn user_agent(req: &Request) -> Option<String> {
    req.headers()
        .get(poem::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

use async_trait::async_trait;
use nest_rs_config::env_var;
use nest_rs_core::Layer;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::{Request, Response, Result};

#[cfg(feature = "otlp")]
use {
    opentelemetry::global,
    opentelemetry::trace::TraceContextExt,
    opentelemetry_http::HeaderExtractor,
    poem::Body,
    poem::http::{HeaderName, HeaderValue},
    std::time::Instant,
    tracing::Instrument,
    tracing_opentelemetry::OpenTelemetrySpanExt,
};

use crate::access_log::parse_access_log_flag;
#[cfg(feature = "otlp")]
use crate::access_log::{AccessLog, AccessLogBody};

/// Per-request HTTP observation.
///
/// Opens a `tracing` span with OTel HTTP semantic-convention attributes,
/// parents it on an incoming W3C `traceparent`, surfaces the trace id as
/// `X-Trace-Id`. The span is exported but **not** rendered in the console
/// (`FmtSpan::NONE`).
///
/// One visible access event per request (`tracing::info!` on target
/// `nest_rs::access`) emitted at **end-of-body**, so `bytes` and `duration_ms`
/// are exact — poem stamps `Content-Length` past this interceptor, so the
/// body is wrapped in a byte-counting stream (see [`crate::access_log`]).
///
/// Toggle via `NESTRS_HTTP__ACCESS_LOG` (default `true`); the OTel span is
/// always created so propagation and OTLP export keep working.
#[interceptor]
#[derive(Clone, Copy, Debug)]
pub(crate) struct OpenTelemetryHttp {
    access_log: bool,
}

impl Default for OpenTelemetryHttp {
    fn default() -> Self {
        Self {
            access_log: parse_access_log_flag(env_var("NESTRS_HTTP__ACCESS_LOG").as_deref()),
        }
    }
}

#[cfg(feature = "otlp")]
const X_TRACE_ID: HeaderName = HeaderName::from_static("x-trace-id");

impl Layer for OpenTelemetryHttp {}

#[async_trait]
impl Interceptor for OpenTelemetryHttp {
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
            // OpenTelemetryHttp is the outermost discovered interceptor, so swallowing
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

#[cfg(test)]
mod tests {
    #[cfg(feature = "otlp")]
    mod otlp {
        use nest_rs_interceptors::InterceptorExt;
        use poem::http::{Method, StatusCode, header};
        use poem::{Endpoint, IntoResponse, Request, endpoint::make};

        use super::super::{OpenTelemetryHttp, client_ip, current_trace_id, user_agent};

        #[test]
        fn user_agent_reads_the_header_when_present() {
            let mut req = Request::default();
            req.headers_mut()
                .insert(header::USER_AGENT, "curl/8.0".parse().unwrap());
            assert_eq!(user_agent(&req).as_deref(), Some("curl/8.0"));
        }

        #[test]
        fn user_agent_is_none_when_header_absent() {
            assert!(user_agent(&Request::default()).is_none());
        }

        #[test]
        fn user_agent_ignores_non_utf8_header() {
            let mut req = Request::default();
            req.headers_mut().insert(
                header::USER_AGENT,
                poem::http::HeaderValue::from_bytes(&[0xff, 0xfe, 0xfd]).unwrap(),
            );
            assert!(
                user_agent(&req).is_none(),
                "non-utf8 ua must not panic or surface"
            );
        }

        #[test]
        fn client_ip_falls_back_to_display_when_remote_is_not_a_socket_addr() {
            // A default `Request` has a non-TCP remote (used by poem's tests);
            // the helper must still produce a non-empty string instead of panicking.
            let s = client_ip(&Request::default());
            assert!(!s.is_empty(), "client_ip must always produce a value");
        }

        #[test]
        fn current_trace_id_is_none_for_an_unconnected_span() {
            // A bare `tracing::Span` with no OTel context attached has no valid
            // span_context, so the helper must return None rather than emit a
            // zero trace id.
            let span = tracing::info_span!("test");
            assert!(current_trace_id(&span).is_none());
        }

        async fn read_body_to_bytes(resp: poem::Response) -> Vec<u8> {
            resp.into_body().into_vec().await.expect("body bytes")
        }

        #[tokio::test]
        async fn intercept_attaches_x_trace_id_header_to_response_when_traceparent_present() {
            let endpoint = make(|_req: Request| async { "hello-world".into_response() });
            let wrapped = endpoint.interceptor(OpenTelemetryHttp { access_log: true });

            // A W3C traceparent the propagator accepts: trace id all-1s, span id all-1s.
            let req = Request::builder()
                .method(Method::GET)
                .uri("/path".parse().unwrap())
                .header(
                    "traceparent",
                    "00-11111111111111111111111111111111-1111111111111111-01",
                )
                .header(header::USER_AGENT, "curl/8.0")
                .finish();
            let resp = wrapped
                .call(req)
                .await
                .expect("handler runs")
                .into_response();
            assert_eq!(resp.status(), StatusCode::OK);
            // The interceptor sets X-Trace-Id whenever a valid span context exists;
            // with the propagator extracting the traceparent, it must be present.
            let header = resp.headers().get("x-trace-id").cloned();
            // Body still needs to be drained so the AccessLogBody stream emits.
            let _ = read_body_to_bytes(resp).await;
            assert!(
                header.is_some(),
                "X-Trace-Id must surface a valid trace id when traceparent is provided",
            );
        }

        #[tokio::test]
        async fn intercept_emits_response_without_traceparent_too() {
            // No traceparent header: the interceptor's fast path skips the
            // propagator extraction, but everything else (status recording,
            // body wrapping, access log) must still run.
            let endpoint = make(|_req: Request| async { "ok".into_response() });
            let wrapped = endpoint.interceptor(OpenTelemetryHttp { access_log: true });

            let req = Request::builder()
                .method(Method::GET)
                .uri("/no-tp".parse().unwrap())
                .finish();
            let resp = wrapped
                .call(req)
                .await
                .expect("handler runs")
                .into_response();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = read_body_to_bytes(resp).await;
            assert_eq!(body, b"ok");
        }

        #[tokio::test]
        async fn intercept_records_status_for_an_error_response() {
            // The handler returns an Err; the interceptor must render it into
            // a Response so the status is recorded on the span and the access
            // log still fires.
            let endpoint = make(|_req: Request| async {
                Err::<&'static str, _>(poem::Error::from_status(StatusCode::IM_A_TEAPOT))
            });
            let wrapped = endpoint.interceptor(OpenTelemetryHttp { access_log: true });

            let req = Request::builder()
                .method(Method::POST)
                .uri("/teapot".parse().unwrap())
                .finish();
            let resp = wrapped
                .call(req)
                .await
                .expect("interceptor normalises err")
                .into_response();
            assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
            let _ = read_body_to_bytes(resp).await;
        }

        #[tokio::test]
        async fn intercept_skips_access_log_when_flag_is_off() {
            // access_log = false still wraps the body and records the status,
            // but skips the tracing::info! emission. Exercising the false
            // branch covers the second arm of `AccessLog::emit`.
            let endpoint = make(|_req: Request| async { "silent".into_response() });
            let wrapped = endpoint.interceptor(OpenTelemetryHttp { access_log: false });

            let req = Request::builder()
                .method(Method::GET)
                .uri("/quiet".parse().unwrap())
                .finish();
            let resp = wrapped
                .call(req)
                .await
                .expect("handler runs")
                .into_response();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = read_body_to_bytes(resp).await;
            assert_eq!(body, b"silent");
        }

        #[tokio::test]
        async fn intercept_drops_body_without_panicking_when_client_disconnects() {
            // Drop-on-disconnect: build the Response, then drop the body
            // without polling to end-of-stream. `AccessLogBody::Drop` must
            // run `emit_once` exactly once.
            let endpoint = make(|_req: Request| async { "abandoned".into_response() });
            let wrapped = endpoint.interceptor(OpenTelemetryHttp { access_log: true });

            let req = Request::builder()
                .method(Method::GET)
                .uri("/dropped".parse().unwrap())
                .finish();
            let resp = wrapped
                .call(req)
                .await
                .expect("handler runs")
                .into_response();
            // Drop without reading — exercises AccessLogBody::Drop.
            drop(resp);
        }

        #[tokio::test]
        async fn intercept_accumulates_byte_count_across_chunked_body() {
            // A non-empty body must be polled to completion so the byte counter
            // accumulates, then `emit_once` runs on the terminal poll.
            let payload = "a".repeat(2048);
            let payload_for_handler = payload.clone();
            let endpoint = make(move |_req: Request| {
                let body = payload_for_handler.clone();
                async move { body.into_response() }
            });
            let wrapped = endpoint.interceptor(OpenTelemetryHttp { access_log: true });

            let req = Request::builder()
                .method(Method::GET)
                .uri("/big".parse().unwrap())
                .finish();
            let resp = wrapped
                .call(req)
                .await
                .expect("handler runs")
                .into_response();
            let body = read_body_to_bytes(resp).await;
            assert_eq!(body.len(), payload.len());
        }
    }
}

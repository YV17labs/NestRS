//! Structured access logging — emits one `tracing::info!` line on target
//! `nest_rs::access` per request at end-of-body, so `bytes` and `duration_ms`
//! are exact. Toggle via `NESTRS_HTTP__ACCESS_LOG` (default `true`).
//!
//! The body-counting wrapper is the visible mechanism: poem stamps
//! `Content-Length` past the OpenTelemetry interceptor, so the response body
//! is wrapped in a byte-counting stream that fires the access event when the
//! stream finishes (or is dropped on client disconnect).

#[cfg(feature = "otlp")]
use {
    bytes::Bytes,
    futures_core::Stream,
    std::io::Error as IoError,
    std::pin::Pin,
    std::task::{Context, Poll},
    std::time::Instant,
};

/// Parse the access-log toggle. Default ON; only literal falsy values
/// (`0`/`false`/`off`/`no`, case-insensitive) turn it off — every other
/// value (including `"1"`, `"yes"`, garbage) stays on, so a typo cannot
/// silently disable observability.
pub(crate) fn parse_access_log_flag(raw: Option<&str>) -> bool {
    // Default ON: absent or unrecognized (`None` from `parse_bool`) stays on, so
    // a typo cannot silently disable observability; only an explicit falsy value
    // turns it off.
    raw.and_then(crate::config::parse_bool).unwrap_or(true)
}

#[cfg(feature = "otlp")]
pub(crate) struct AccessLog {
    pub method: poem::http::Method,
    pub path: String,
    pub status: u16,
    pub client_ip: String,
    pub user_agent: String,
    pub trace_id: String,
    pub start: Instant,
    pub span: tracing::Span,
    pub access_log: bool,
}

#[cfg(feature = "otlp")]
impl AccessLog {
    fn emit(self, bytes: u64) {
        self.span.record("http.response.body.size", bytes);
        if self.access_log {
            let duration_ms = (self.start.elapsed().as_secs_f64() * 1e6).round() / 1e3;
            tracing::info!(
                target: "nest_rs::access",
                method = %self.method,
                path = %self.path,
                status = self.status,
                bytes = bytes,
                duration_ms = duration_ms,
                client_ip = %self.client_ip,
                user_agent = %self.user_agent,
                trace_id = %self.trace_id,
                "request served",
            );
        }
    }
}

#[cfg(feature = "otlp")]
pub(crate) struct AccessLogBody {
    pub inner: Pin<Box<dyn Stream<Item = Result<Bytes, IoError>> + Send>>,
    pub counted: u64,
    pub log: Option<AccessLog>,
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

#[cfg(test)]
mod tests {
    use super::*;

    // The access-log flag is the on/off switch for every request's
    // `nest_rs::access` line. A regression here turns prod blind, so the
    // parsing rules are pinned exhaustively.
    #[test]
    fn unset_env_keeps_access_log_on() {
        assert!(parse_access_log_flag(None));
    }

    #[test]
    fn canonical_falsy_values_turn_access_log_off() {
        for raw in ["0", "false", "off", "no"] {
            assert!(
                !parse_access_log_flag(Some(raw)),
                "expected off for {raw:?}"
            );
        }
    }

    #[test]
    fn falsy_values_are_case_and_whitespace_tolerant() {
        for raw in ["  FALSE  ", "Off", "NO", "0\n"] {
            assert!(
                !parse_access_log_flag(Some(raw)),
                "expected off for {raw:?}"
            );
        }
    }

    #[test]
    fn truthy_and_unknown_values_keep_access_log_on() {
        // `"1"`, `"yes"`, garbage, an empty string — all preserve the default.
        // Typos must not silently disable observability.
        for raw in ["", "1", "true", "yes", "on", "garbage", "FaLsy"] {
            assert!(parse_access_log_flag(Some(raw)), "expected on for {raw:?}");
        }
    }
}

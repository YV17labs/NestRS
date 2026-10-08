//! Server-Sent Events — the response shape `#[sse]` mounts.
//!
//! A `#[sse]` route returns a stream of [`SseEvent`]: the decorator turns it
//! into the `text/event-stream` response, applies the keep-alive interval, and
//! closes the stream at `<PREFIX>_HTTP__SSE_MAX_CONNECTION_SECS` (4 hours by
//! default, `0` ⇒ unlimited) — a stream authenticates once, so the ceiling
//! bounds stale privileges.
//!
//! The ceiling bounds *emission*, not the socket: a peer that stops reading
//! parks the write and keeps the connection alive, and poem 3.1 / hyper 1 offer
//! no per-response write deadline. Bound idle sockets at the server
//! (`Server::idle_timeout`) or the proxy.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use nest_rs_core::Container;
use poem::web::sse::SSE;
use poem::{IntoResponse, Response};

use crate::config::HttpConfig;

/// One event on a `text/event-stream`: a payload, and optionally an event type,
/// an id a reconnecting client sends back as `Last-Event-ID`, and a retry hint.
pub use poem::web::sse::Event as SseEvent;

/// What an `#[sse]` handler returns: `-> SseStream`, or
/// `-> Result<SseStream, E>` when opening the stream can fail.
///
/// A named type because an `async fn` on `&self` returning `impl Stream`
/// captures `&self` under the 2024 capture rules, so it is not `'static`.
pub struct SseStream(BoxStream<'static, SseEvent>);

impl SseStream {
    /// Take ownership of any stream of events.
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = SseEvent> + Send + 'static,
    {
        Self(stream.boxed())
    }
}

impl fmt::Debug for SseStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SseStream(..)")
    }
}

/// The two long-lived-connection controls a `#[sse]` route is mounted with,
/// read once from [`HttpConfig`] at mount time.
#[derive(Clone, Copy, Debug)]
pub struct SseSettings {
    keep_alive: Option<Duration>,
    max_connection: Option<Duration>,
}

impl SseSettings {
    /// Resolve from the app's [`HttpConfig`], or its defaults when the
    /// container has none (it then serves no HTTP).
    pub fn resolve(container: &Container) -> Self {
        let config = container
            .get::<HttpConfig>()
            .unwrap_or_else(|| Arc::new(HttpConfig::default()));
        Self {
            keep_alive: config.sse_keep_alive,
            max_connection: config.sse_max_connection,
        }
    }

    /// Wrap a handler's event stream into the response the route serves —
    /// marked [`OpenEndedBody`](crate::OpenEndedBody), so the transport ends it
    /// at the shutdown signal. Emitted by `#[sse]`.
    pub fn respond(&self, stream: SseStream) -> Response {
        let stream = stream.0;
        // A ceiling, not an idle timeout: traffic never pushes it out.
        let sse = match self.max_connection {
            Some(ttl) => SSE::new(stream.take_until(async move {
                tokio::time::sleep(ttl).await;
                tracing::info!(
                    target: crate::target::HTTP,
                    max_connection_secs = ttl.as_secs(),
                    "closing sse stream: max lifetime reached",
                );
            })),
            None => SSE::new(stream),
        };
        let sse = match self.keep_alive {
            Some(every) => sse.keep_alive(every),
            None => sse,
        };
        let mut response = sse.into_response();
        response.extensions_mut().insert(crate::OpenEndedBody);
        response
    }
}

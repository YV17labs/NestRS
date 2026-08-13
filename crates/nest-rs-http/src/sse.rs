//! Server-Sent Events — the response shape `#[sse]` mounts.
//!
//! A `#[sse]` route returns a stream of [`SseEvent`] and nothing else: the
//! decorator turns it into the `text/event-stream` response, applies the
//! keep-alive comment interval, and closes the stream when the connection
//! ceiling elapses. The route is a plain `GET` on the HTTP transport — the same
//! guard chain, the same pipes, the same `#[api]` metadata as any other route.
//!
//! **The ceiling is the same security control WS and graphql-ws carry**, for the
//! same reason and under the same name. A stream authenticates **once**, when
//! the request arrives, and then emits for as long as it likes: without a
//! ceiling it keeps the caller's privileges after the bearer token has expired,
//! the user has logged out, or the grant was revoked.
//! `NESTRS_HTTP__SSE_MAX_CONNECTION_SECS` bounds that, with the same 4-hour
//! default and the same `0` ⇒ unlimited spelling as
//! `NESTRS_WS__MAX_CONNECTION_SECS` and `NESTRS_GRAPHQL__MAX_CONNECTION_SECS`.
//!
//! **What it bounds is *emission*, and the connection is a reported gap.** The
//! deadline is composed into the response body, so it is evaluated whenever that
//! body is polled: no event is ever produced past the ceiling, at any rate a
//! peer trickles at — which is the stale-privilege window that matters, and it
//! is measured, not assumed. What outlives it is the **socket**: a peer that
//! stops reading parks the write, and hyper's connection task, its buffers and
//! everything the stream holds stay alive until that peer reads again or the
//! socket dies.
//!
//! **Closing that from inside this crate was tried twice and is not sound**, and
//! the reason is structural rather than a missing effort. The only thing still
//! polled while a write is parked is the socket — and a socket does not know
//! which *response* its bytes belong to. Bounding it there truncated unrelated
//! traffic: a full-speed client downloading 4 MB over a connection that had
//! carried a stream received 1.4 MB under a declared `content-length`, which is
//! a protocol lie told to a well-behaved peer, and HTTP/2 multiplexes an
//! origin's whole traffic onto that one socket. The information needed — *which
//! response is stalled* — lives in the body, which is precisely what stops being
//! polled. poem 3.1 and hyper 1 expose no per-response write deadline and no
//! abort handle usable while parked, so there is nothing here to reach for.
//!
//! **Bound idle sockets at the server or the proxy** until there is: poem's own
//! `Server::idle_timeout` closes a connection with no traffic, which is the
//! shape that can be enforced without knowing whose bytes are queued.
//!
//! The variable sits in the **`http`** namespace rather than an `sse` one of its
//! own, and that is the one deliberate difference from its two peers: SSE is a
//! response shape of the HTTP transport, not a module. It owns no `#[config]`,
//! so under *one seam per config* it gets no `for_root` — `HttpModule::for_root`
//! is already its in-code path.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use nest_rs_core::Container;
use poem::web::sse::SSE;

use crate::config::HttpConfig;

/// One event on a `text/event-stream`: a payload, and optionally an event type,
/// an id a reconnecting client sends back as `Last-Event-ID`, and a retry hint.
///
/// Re-exported so a controller streaming events declares no transport crate of
/// its own — the same reason `#[input]` and `Json` are reached through this
/// crate rather than through poem.
pub use poem::web::sse::Event as SseEvent;

/// What an `#[sse]` handler returns: `-> SseStream`, or
/// `-> Result<SseStream, E>` when opening the stream can fail.
///
/// **A named type rather than `impl Stream<Item = SseEvent>`**, and that is the
/// whole reason it exists. An `async fn` on `&self` returning an opaque type
/// captures the `&self` lifetime under the 2024 capture rules, so the opaque
/// stream is not `'static` and cannot outlive the call — which is exactly what a
/// response body must do. The developer would have to write
/// `impl Stream<Item = SseEvent> + use<>` and know why. They write
/// [`SseStream::new`] instead, and the boxing that makes it `'static` happens
/// once, where the stream is built.
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
/// read once from [`HttpConfig`] at mount time rather than per request: both are
/// fixed for the life of the process, and a per-request container lookup on a
/// route whose whole job is to stay open would be pure overhead.
#[derive(Clone, Copy, Debug)]
pub struct SseSettings {
    keep_alive: Option<Duration>,
    max_connection: Option<Duration>,
}

impl SseSettings {
    /// Resolve from the app's [`HttpConfig`].
    ///
    /// `#[routes]` emits this only for a controller that carries a `#[sse]`
    /// route.
    ///
    /// A container with no `HttpConfig` cannot be serving HTTP, so the defaults
    /// stand in rather than panicking: this runs inside `Controller::mount`, and
    /// failing a boot here would report the wrong cause.
    pub fn resolve(container: &Container) -> Self {
        let config = container
            .get::<HttpConfig>()
            .unwrap_or_else(|| Arc::new(HttpConfig::default()));
        Self {
            keep_alive: config.sse_keep_alive,
            max_connection: config.sse_max_connection,
        }
    }

    /// Wrap a handler's event stream into the response the route serves.
    ///
    /// Emitted by `#[sse]`; there is no reason to call it by hand, and doing so
    /// on an ordinary route would produce a `text/event-stream` the document
    /// does not describe.
    pub fn respond(&self, stream: SseStream) -> SSE {
        let stream = stream.0;
        // A ceiling, not an idle timeout: traffic never pushes it out. It is
        // evaluated whenever the body is polled, so it bounds emission — see the
        // module docs for what that does and does not cover.
        let sse = match self.max_connection {
            Some(ttl) => SSE::new(stream.take_until(async move {
                tokio::time::sleep(ttl).await;
                tracing::info!(
                    target: "nest_rs::http",
                    max_connection_secs = ttl.as_secs(),
                    "closing sse stream: max lifetime reached",
                );
            })),
            None => SSE::new(stream),
        };
        match self.keep_alive {
            Some(every) => sse.keep_alive(every),
            None => sse,
        }
    }
}

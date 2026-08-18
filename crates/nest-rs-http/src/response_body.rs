//! The response body wrapper — what keeps a request alive until its last byte.
//!
//! # A handler returning is not the request ending
//!
//! An `async fn` handler returns a [`Response`]; hyper writes its body
//! afterwards, on the connection task, with every task-local the transport edge
//! installed already unwound and the operation span already exited. For a body
//! that is `Bytes` in hand that gap is invisible. For a body that is a
//! **stream** — `#[sse]`, a download, anything a handler builds over a
//! `Stream` — the stream's own code runs in that gap, and it is still serving
//! the request the handler was serving.
//!
//! So without this wrapper an SSE stream emits its events under no span and no
//! ambient context at all: `trace_id` missing from every line, `actor_id`
//! missing, `current_trace_id()` answering `None` inside the developer's own
//! stream. That is the correlation primitive being true for short responses and
//! false for long ones, which is worse than it being absent — the capability
//! reads as present.
//!
//! [`carry`] therefore wraps **every** non-empty body, whatever the access log
//! is set to. Correlation cannot be optional, and a config flag is a weaker
//! condition than a crate: `HttpConfig.access_log = false` turns a *log line*
//! off, never the identity of the work.
//!
//! # Counting the body without changing how it is framed
//!
//! hyper picks `Content-Length` over `Transfer-Encoding: chunked` from
//! `size_hint().exact()`. Wrapping a response through poem's only public stream
//! constructor (`Body::from_bytes_stream`) produces a `StreamBody`, whose size
//! hint is the trait default `(0, None)` — so a byte counter written that way
//! turns **every** response chunked, silently, to report a log field.
//!
//! [`Carried`] is therefore a real `http_body::Body` that forwards `size_hint`
//! and `is_end_stream`: the framing a handler's response would have had is the
//! framing it gets. poem's public `From<Body>` / `From<BoxBody>` impls are what
//! let the wrapper sit inside a [`Body`] at all.

use std::io::Error as IoError;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use poem::{Body, Response};

use nest_rs_core::RequestContinuation;

use crate::access_log::AccessLog;

/// The exact type poem converts a [`Body`] to and from in its public `From`
/// impls. Naming it is what lets the wrapper be an `http_body::Body` rather than
/// a stream — see the module doc.
type PoemBody = BoxBody<Bytes, IoError>;

/// Attach the request to its own response body, and file the access line when
/// the body ends.
///
/// A body with nothing left to write — a `204`, a `304`, most refusals — is
/// handed back untouched: there is no code left to run inside it, so there is
/// nothing to carry a context *for*, and the line is filed here.
pub(crate) fn carry(
    continuation: RequestContinuation,
    span: tracing::Span,
    log: Option<AccessLog>,
    resp: Response,
) -> Response {
    let status = resp.status().as_u16();
    let (parts, body) = resp.into_parts();
    if body.is_empty() {
        span.record("http.response.body.size", 0);
        if let Some(log) = log {
            log.emit(status, 0);
        }
        return Response::from_parts(parts, body);
    }
    let carried = Carried {
        inner: body.into(),
        counted: 0,
        continuation,
        span,
        pending: log.map(|log| Pending { log, status }),
    };
    Response::from_parts(parts, Body::from(PoemBody::new(carried)))
}

/// A line waiting on the body it will report the size of. `None` on
/// [`Carried`] when the access log is off — the body is still carried, because
/// the context is not the log's.
struct Pending {
    log: AccessLog,
    status: u16,
}

/// A response body written under the request that produced it.
///
/// `size_hint` and `is_end_stream` are forwarded verbatim; see the module doc
/// for why that is the load-bearing part rather than a courtesy.
struct Carried {
    inner: PoemBody,
    counted: u64,
    /// Re-installed around every poll, so the stream's own code reads the same
    /// ambient answers the handler read.
    continuation: RequestContinuation,
    /// Entered around every poll, so the stream's own *events* are rooted at the
    /// request rather than at the connection task — and held open until the body
    /// ends, so `http.response.body.size` lands on a span an exporter still has.
    span: tracing::Span,
    pending: Option<Pending>,
}

impl Carried {
    /// At most once — the stream ending and the body being dropped both reach
    /// here, and a request is filed one time or the count is meaningless.
    fn file(&mut self) {
        // The size lands on the span whether or not a line is filed: it is what
        // an exported server span is read for, and the span is held open until
        // here precisely so it can.
        self.span.record("http.response.body.size", self.counted);
        if let Some(pending) = self.pending.take() {
            pending.log.emit(pending.status, self.counted);
        }
    }
}

impl HttpBody for Carried {
    type Data = Bytes;
    type Error = IoError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        // Span and context together, exactly as the edge installed them around
        // the handler: an event the stream emits carries `trace_id` from the
        // span, and a `current_trace_id()` inside the stream reads the
        // task-local. Neither substitutes for the other.
        //
        // Both are left behind before the line is filed: the access log carries
        // its own fields and is nobody's child event.
        let polled = {
            let Carried {
                inner,
                continuation,
                span,
                ..
            } = &mut *this;
            let _entered = span.enter();
            continuation.enter(|| Pin::new(&mut *inner).poll_frame(cx))
        };
        match polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.counted += data.len() as u64;
                }
                Poll::Ready(Some(Ok(frame)))
            }
            terminal @ Poll::Ready(_) => {
                this.file();
                terminal
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for Carried {
    fn drop(&mut self) {
        self.file();
    }
}

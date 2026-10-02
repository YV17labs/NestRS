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
//!
//! # The way down
//!
//! A body is still the request running, so the shutdown window treats it as
//! one: a body with an end of its own — a download — gets the window, and is
//! cut at its close. A body with **no** end of its own — an event stream — would
//! only ever spend the whole window waiting for an end that is not coming, so a
//! response marked [`OpenEndedBody`] is ended at the shutdown signal instead:
//! cleanly, its last chunk written, so a client's `EventSource` reads an end and
//! reconnects rather than reading a cut. Either way the unit did not settle on
//! its own, and its line says so — the head's `status`, the `bytes` written, and
//! `outcome = cancelled`.

use std::future::Future;
use std::io::Error as IoError;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use poem::{Body, Response};
use tokio_util::sync::WaitForCancellationFutureOwned;

use nest_rs_core::RequestContinuation;

use crate::access_log::AccessLog;
use crate::drain::Drain;

/// Marks a response whose body has no end of its own — an event stream, a
/// push channel — so the transport ends it at the shutdown signal rather than
/// holding the window open for an end that is not coming, and files its line
/// `cancelled`.
///
/// A response extension: `response.extensions_mut().insert(OpenEndedBody)`.
/// An `#[sse]` route is marked already, and so is the MCP edge's standalone
/// stream; a handler streaming events by hand marks its own. A body that does
/// end — a download, however long — is not marked: it gets the window, like a
/// request still running.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenEndedBody;

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
    drain: &Arc<Drain>,
) -> Response {
    let status = resp.status().as_u16();
    let (parts, body) = resp.into_parts();
    if body.is_empty() {
        span.record("http.response.body.size", 0);
        if let Some(log) = log {
            // Inside the context, outside the span — see `Carried::file`.
            continuation.enter(|| log.emit(status, 0, None));
        }
        return Response::from_parts(parts, body);
    }
    // Boxed only for the responses that need it: a pinned wait, per open-ended
    // stream, rather than a field every response pays for.
    let going_away = parts
        .extensions
        .get::<OpenEndedBody>()
        .map(|_| Box::pin(drain.going_away()));
    let carried = Carried {
        inner: body.into(),
        counted: 0,
        continuation,
        span,
        pending: log.map(|log| Pending { log, status }),
        drain: Arc::clone(drain),
        going_away,
        reached_end: false,
        ended_at_signal: false,
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
    /// The transport's way down — read when the body is dropped unfinished, to
    /// tell a cut by the window from a body that simply ended.
    drain: Arc<Drain>,
    /// The shutdown signal, for an [`OpenEndedBody`] only.
    going_away: Option<Pin<Box<WaitForCancellationFutureOwned>>>,
    /// The inner body yielded its last frame.
    reached_end: bool,
    /// The signal ended an [`OpenEndedBody`].
    ended_at_signal: bool,
}

impl Carried {
    /// At most once — the stream ending and the body being dropped both reach
    /// here, and a request is filed one time or the count is meaningless.
    fn file(&mut self) {
        // The size lands on the span whether or not a line is filed: it is what
        // an exported server span is read for, and the span is held open until
        // here precisely so it can.
        self.span.record("http.response.body.size", self.counted);
        let counted = self.counted;
        let outcome = self
            .stopped_unsettled()
            .then_some(nest_rs_core::operation_log::CANCELLED);
        // The span says how the unit ended in the line's word, whatever the
        // access log is set to — it is the span a backend counts failures on.
        if let Some(outcome) = outcome {
            nest_rs_core::operation_log::record_outcome(&self.span, outcome);
        }
        if let Some(pending) = self.pending.take() {
            // **The context, not the span.** The line is still nobody's child
            // event — entering the span would file it under the request it
            // reports on — but its `trace_id` / `span_id` / `actor_id` come off
            // the ambient correlation like every other line's, rather than being
            // spelled a second time as event fields. One source, one position in
            // the JSON envelope.
            self.continuation
                .enter(|| pending.log.emit(pending.status, counted, outcome));
        }
    }

    /// The transport stopped this body before it ended on its own — at the
    /// signal, for an open-ended one, or by closing its connection at the end
    /// of the window.
    ///
    /// Read positively, never inferred from a drop alone: hyper drops a body it
    /// was never going to write — a `HEAD` answer, a `204` — without polling it,
    /// so "dropped before its end" by itself would call those cut. A body the
    /// window cut is dropped once the window has closed; nothing else is.
    fn stopped_unsettled(&self) -> bool {
        self.ended_at_signal
            || (!self.reached_end && !self.inner.is_end_stream() && self.drain.is_past_bound())
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
        // Ended stays ended: the stream beneath still has events to give, and a
        // body that said `None` must not hand one out after it.
        if this.ended_at_signal {
            return Poll::Ready(None);
        }
        // The signal ends a body with no end of its own, here rather than in the
        // stream: what it is, is the response's to say, and the transport's to
        // act on. Checked before the stream so an end is not raced by one more
        // event.
        if let Some(going_away) = this.going_away.as_mut()
            && going_away.as_mut().poll(cx).is_ready()
        {
            this.going_away = None;
            this.ended_at_signal = true;
            this.file();
            return Poll::Ready(None);
        }
        // Span and context together, exactly as the edge installed them around
        // the handler: an event the stream emits carries `trace_id` from the
        // span, and a `current_trace_id()` inside the stream reads the
        // task-local. Neither substitutes for the other.
        //
        // The span is left behind before the line is filed — it is nobody's
        // child event — while the context is re-entered around it, so the ids on
        // that line come from where every other line's come from.
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
                this.reached_end = true;
                this.file();
                terminal
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.ended_at_signal || self.inner.is_end_stream()
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use http_body_util::BodyExt;
    use nest_rs_core::Correlation;

    use super::*;

    /// An event stream that never runs dry, as an `#[sse]` route's does.
    fn open_ended(drain: &Arc<Drain>) -> PoemBody {
        let events = futures_util::stream::repeat_with(|| {
            Ok::<_, IoError>(Bytes::from_static(b"data: tick\n\n"))
        });
        let mut response = Response::builder().body(Body::from_bytes_stream(events));
        response.extensions_mut().insert(OpenEndedBody);
        let carried = carry(
            RequestContinuation::new(None, Correlation::minted(None)),
            tracing::Span::none(),
            None,
            response,
            drain,
        );
        carried.into_body().into()
    }

    /// The signal ends an open-ended body, and it stays ended: polled again, it
    /// does not reach back into the stream it was ended over.
    #[tokio::test]
    async fn an_open_ended_body_ends_at_the_signal_and_stays_ended() {
        let drain = Arc::new(Drain::default());
        let mut body = open_ended(&drain);
        assert!(
            body.frame().await.is_some(),
            "the stream flows before the signal"
        );

        drain.begin(Duration::from_secs(20));

        assert!(body.frame().await.is_none(), "ended at the signal");
        assert!(body.is_end_stream());
        assert!(body.frame().await.is_none(), "and stays ended");
    }
}

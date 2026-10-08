//! The response body wrapper — what keeps a request alive until its last byte.
//!
//! hyper writes a body after the handler returns, with the edge's task-locals
//! unwound and its span exited, so a streamed body (`#[sse]`, a download) would
//! run under no context: [`carry`] wraps every non-empty body, whatever the
//! access log is set to.
//!
//! hyper picks `Content-Length` over chunked from `size_hint().exact()`, and
//! poem's `Body::from_bytes_stream` reports `(0, None)`: [`Carried`] is a real
//! `http_body::Body` forwarding `size_hint` and `is_end_stream`, so the framing
//! is unchanged.
//!
//! A body marked [`OpenEndedBody`] is ended cleanly at the shutdown signal; any
//! other gets the window and is cut at its close. Either way its line files
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
/// An `#[sse]` route is marked already; a body that does end — a download,
/// however long — is not marked.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenEndedBody;

/// The exact type poem converts a [`Body`] to and from in its public `From`
/// impls.
type PoemBody = BoxBody<Bytes, IoError>;

/// Attach the request to its own response body, and file the access line when
/// the body ends.
///
/// A body with nothing left to write — a `204`, a `304` — is handed back
/// untouched, and the line is filed here.
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
            continuation.enter(|| log.emit(&span, status, 0, None));
        }
        return Response::from_parts(parts, body);
    }
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

/// A line waiting on the body it will report the size of.
struct Pending {
    log: AccessLog,
    status: u16,
}

/// A response body written under the request that produced it.
///
/// `size_hint` and `is_end_stream` are forwarded verbatim: hyper frames the
/// response from them.
struct Carried {
    inner: PoemBody,
    counted: u64,
    /// Re-installed around every poll.
    continuation: RequestContinuation,
    /// Entered around every poll, and held open until the body ends so
    /// `http.response.body.size` lands on a span an exporter still has.
    span: tracing::Span,
    pending: Option<Pending>,
    /// Read when the body is dropped unfinished, to tell a cut by the window.
    drain: Arc<Drain>,
    /// The shutdown signal, for an [`OpenEndedBody`] only.
    going_away: Option<Pin<Box<WaitForCancellationFutureOwned>>>,
    /// The inner body yielded its last frame.
    reached_end: bool,
    /// The signal ended an [`OpenEndedBody`].
    ended_at_signal: bool,
}

impl Carried {
    /// Files the line at most once: the stream ending and the body being
    /// dropped both reach here.
    fn file(&mut self) {
        self.span.record("http.response.body.size", self.counted);
        let counted = self.counted;
        let outcome = self
            .stopped_unsettled()
            .then_some(nest_rs_core::operation_log::CANCELLED);
        match self.pending.take() {
            // The context, not the span: the line is nobody's child event.
            Some(pending) => self.continuation.enter(|| {
                pending
                    .log
                    .emit(&self.span, pending.status, counted, outcome)
            }),
            None => {
                if let Some(outcome) = outcome {
                    nest_rs_core::operation_log::record_outcome(&self.span, outcome);
                }
            }
        }
    }

    /// The transport stopped this body before it ended on its own, at the
    /// signal or at the window's close.
    ///
    /// Never inferred from a drop alone: hyper drops a body it never writes (a
    /// `HEAD` answer, a `204`) without polling it.
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
        // A body that said `None` must not hand out the stream's next event.
        if this.ended_at_signal {
            return Poll::Ready(None);
        }
        // Checked before the stream, so an end is not raced by one more event.
        if let Some(going_away) = this.going_away.as_mut()
            && going_away.as_mut().poll(cx).is_ready()
        {
            this.going_away = None;
            this.ended_at_signal = true;
            this.file();
            return Poll::Ready(None);
        }
        // Both: an event reads `trace_id` off the span, `current_trace_id()`
        // reads the task-local.
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

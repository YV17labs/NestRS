//! The response body wrapper — what keeps a request alive until its last byte.
//!
//! hyper writes a body after the handler returns, with the edge's task-locals
//! unwound and its span exited, so a streamed body (`#[sse]`, a download) would
//! run under no context: [`carry`] wraps every non-empty body, whatever the
//! access log is set to. A streaming body a route returned runs under what its
//! handler ran under, captured as a `Body` continuation where it returned
//! ([`continue_body`]): the caller's ability, the pool rather than the
//! transaction the route settles.
//!
//! hyper picks `Content-Length` over chunked from `size_hint().exact()`, and a
//! streamed body reports `(0, None)`: [`Carried`] is a real `http_body::Body`
//! forwarding `size_hint` and `is_end_stream`, so the framing is unchanged.
//!
//! A body marked [`OpenEndedBody`] is ended cleanly at the shutdown signal; any
//! other gets the window and is cut at its close. Either way its line files
//! `outcome = cancelled`. A body whose poll unwinds ends there, with an error —
//! the framing cannot be finished, so HTTP/1 closes its connection and HTTP/2
//! resets its stream alone — and its line files `outcome = panic`.

use std::any::Any;
use std::future::{Future, poll_fn};
use std::io::Error as IoError;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use tokio_util::sync::WaitForCancellationFutureOwned;

use nest_rs_core::operation_log::{CANCELLED, PANIC};
use nest_rs_core::{Continuation, RequestContinuation};

use crate::access_log::AccessLog;
use crate::body::Body;
use crate::drain::Drain;
use crate::response::Response;

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

/// What a streaming body continues its request under: what the handler ran
/// under, captured where it returned the body, each value through its own
/// `carry` — the executor as the pool, never the transaction the route settles.
#[derive(Clone)]
struct BodyContinuation(RequestContinuation);

/// Capture, for a body that streams, what the handler's unit carries into it;
/// a body of known length is written at once and needs none.
pub(crate) fn continue_body(mut resp: Response) -> Response {
    let body = resp.take_body();
    let streams = !body.is_end_stream() && body.size_hint().exact().is_none();
    resp.set_body(body);
    if streams && let Some(carried) = RequestContinuation::capture(Continuation::Body) {
        resp.extensions_mut().insert(BodyContinuation(carried));
    }
    resp
}

/// Attach the request to its own response body, and file the access line when
/// the body ends: under what the handler ran under when it returned a
/// streaming body, under `continuation` otherwise. `outcome` is the unit's when
/// it did not settle before answering — a handler the edge caught unwinding.
///
/// A body with nothing left to write — a `204`, a `304` — is handed back
/// untouched, and the line is filed here.
pub(crate) fn carry(
    continuation: RequestContinuation,
    span: tracing::Span,
    log: Option<AccessLog>,
    outcome: Option<&'static str>,
    resp: Response,
    drain: &Arc<Drain>,
) -> Response {
    let status = resp.status().as_u16();
    let (mut parts, body) = resp.into_parts();
    let continuation = parts
        .extensions
        .remove::<BodyContinuation>()
        .map_or(continuation, |body| body.0);
    if body.is_empty() {
        span.record("http.response.body.size", 0);
        match log {
            // Inside the context, outside the span — see `Carried::file`.
            Some(log) => continuation.enter(|| log.emit(&span, status, 0, outcome)),
            None => {
                if let Some(outcome) = outcome {
                    nest_rs_core::operation_log::record_outcome(&span, outcome);
                }
            }
        }
        return Response::from_parts(parts, body);
    }
    let going_away = parts
        .extensions
        .get::<OpenEndedBody>()
        .map(|_| Box::pin(drain.going_away()));
    let carried = Carried {
        inner: body,
        counted: 0,
        continuation,
        span,
        pending: log.map(|log| Pending { log, status }),
        drain: Arc::clone(drain),
        going_away,
        reached_end: false,
        outcome,
        ended: false,
    };
    Response::from_parts(parts, Body::from_boxed(BoxBody::new(carried)))
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
    inner: Body,
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
    /// How the unit ended when it did not settle: the handler's unwind the
    /// edge answered, this body's own, or the signal ending an [`OpenEndedBody`].
    outcome: Option<&'static str>,
    /// The body ended before its inner one did — at the signal, or on a poll
    /// that unwound — and polls it no more.
    ended: bool,
}

impl Carried {
    /// Files the line at most once: the stream ending and the body being
    /// dropped both reach here.
    fn file(&mut self) {
        self.span.record("http.response.body.size", self.counted);
        let counted = self.counted;
        let outcome = self
            .outcome
            .or_else(|| self.cut_at_the_window().then_some(CANCELLED));
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

    /// The transport cut this body at the window's close, before it ended on
    /// its own.
    ///
    /// Never inferred from a drop alone: hyper drops a body it never writes (a
    /// `HEAD` answer, a `204`) without polling it.
    fn cut_at_the_window(&self) -> bool {
        !self.reached_end && !self.inner.is_end_stream() && self.drain.is_past_bound()
    }

    /// End the body at a poll that unwound: the panic said once, inside the
    /// request, and the line filed `panic` with what was written.
    fn unwound(&mut self, payload: &(dyn Any + Send)) {
        self.continuation.enter(|| {
            self.span.in_scope(|| {
                nest_rs_core::contained_panic!(
                    target: crate::target::HTTP,
                    payload,
                    "a response body panicked; it ended unfinished",
                );
            });
        });
        self.ended = true;
        self.outcome = Some(PANIC);
        self.file();
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
        // A body that ended must not hand out the stream's next event.
        if this.ended {
            return Poll::Ready(None);
        }
        // Checked before the stream, so an end is not raced by one more event.
        if let Some(going_away) = this.going_away.as_mut()
            && going_away.as_mut().poll(cx).is_ready()
        {
            this.going_away = None;
            this.ended = true;
            this.outcome.get_or_insert(CANCELLED);
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
            // Contained, so the process hook leaves the panic to this body.
            continuation.enter(|| {
                pin!(nest_rs_core::panic::contain(poll_fn(|cx| {
                    Pin::new(&mut *inner).poll_frame(cx)
                })))
                .poll(cx)
            })
        };
        match polled {
            Poll::Ready(Ok(Some(Ok(frame)))) => {
                if let Some(data) = frame.data_ref() {
                    this.counted += data.len() as u64;
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Ok(terminal)) => {
                this.reached_end = true;
                this.file();
                Poll::Ready(terminal)
            }
            Poll::Ready(Err(payload)) => {
                this.unwound(payload.as_ref());
                Poll::Ready(Some(Err(IoError::other("the response body panicked"))))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.ended || self.inner.is_end_stream()
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
    fn open_ended(drain: &Arc<Drain>) -> Body {
        let events = futures_util::stream::repeat_with(|| {
            Ok::<_, IoError>(Bytes::from_static(b"data: tick\n\n"))
        });
        let mut response = Response::builder().body(Body::from_stream(events));
        response.extensions_mut().insert(OpenEndedBody);
        let carried = carry(
            RequestContinuation::new(None, Correlation::minted(None)),
            tracing::Span::none(),
            None,
            None,
            response,
            drain,
        );
        carried.into_body()
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

    /// One event, then a panic on the next poll: a stream that unwinds mid-flow.
    fn panics_after_one_event() -> Response {
        let mut sent = false;
        let events = futures_util::stream::poll_fn(move |_| {
            assert!(!std::mem::replace(&mut sent, true), "the stream panicked");
            Poll::Ready(Some(Ok::<_, IoError>(Bytes::from_static(b"data: one\n\n"))))
        });
        Response::builder().body(Body::from_stream(events))
    }

    #[tokio::test]
    async fn a_body_that_panics_ends_with_an_error_and_files_its_line_panic() {
        let logs = nest_rs_testing::LogCapture::install();
        let request = crate::Request::builder()
            .uri(http::Uri::from_static("/feed"))
            .finish();
        let carried = carry(
            RequestContinuation::new(None, Correlation::minted(None)),
            tracing::Span::none(),
            Some(AccessLog::open(
                &request,
                &crate::matched::MatchedRoute::default(),
                None,
                std::time::Instant::now(),
            )),
            None,
            panics_after_one_event(),
            &Arc::new(Drain::default()),
        );
        let mut body = carried.into_body();

        assert!(
            matches!(body.frame().await, Some(Ok(_))),
            "the first event flows"
        );
        let ended = body.frame().await;
        let Some(Err(error)) = ended else {
            panic!("the panic ends the body with an error, got {ended:?}");
        };
        assert!(!error.to_string().contains("stream panicked"), "{error}");
        assert!(body.frame().await.is_none(), "nothing is polled past it");
        drop(body);

        let line = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::REQUEST.name(),
        );
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
            "{line:#?}"
        );
        assert_eq!(line.field("status").as_deref(), Some("200"));
        assert_eq!(
            line.field("bytes").as_deref(),
            Some("11"),
            "what was written"
        );
        let errors: Vec<_> = logs
            .events()
            .into_iter()
            .filter(|event| event.level == "error")
            .collect();
        assert_eq!(errors.len(), 1, "one event says the panic: {errors:#?}");
        assert_eq!(
            errors[0].field(nest_rs_core::panic::FIELD).as_deref(),
            Some("the stream panicked"),
        );
    }
}

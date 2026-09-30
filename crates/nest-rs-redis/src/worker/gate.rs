//! [`ThrottleGate`] — a method's fetch held shut while its throttle's window is
//! full.
//!
//! A throttle counts the attempts a method starts per window, across every
//! replica, and an attempt over the limit is handed back for when the window
//! ends. Without a gate, the replica goes on fetching: every poll brings up to
//! the method's `concurrency` jobs, each is admitted, refused and filed back —
//! about twenty Redis calls a job — and at the window's end the whole backlog
//! falls due at once and goes round again. A window then costs the fetch rate
//! times its length, or the backlog when that is less, to start `limit` jobs.
//!
//! So the first refusal shuts the gate until the window ends — the instant the
//! admission that refused already read off the throttle's key — and while it is
//! shut the worker is not ready: apalis fetches only from a ready worker, so
//! the jobs stay on the queue, where every peer and the autoscaler see them.
//! Jobs the last fetch brought wait in the replica until the gate opens, and
//! are admitted then rather than handed back. A window then costs each replica
//! one fetch's worth of refusals, whatever the backlog.
//!
//! A drain opens the gate: jobs still waiting in the replica are handed back
//! rather than kept past its stop.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::time::{Instant, Sleep};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tower::{Layer, Service};

/// When one method's fetch opens again, shared by its deliveries — which shut
/// it — and its worker's readiness, which reads it.
pub(crate) struct ThrottleGate {
    /// The instant the current window ends, while it is full.
    shut_until: Mutex<Option<Instant>>,
    /// The shutdown began: the gate opens for good.
    draining: CancellationToken,
}

impl ThrottleGate {
    /// An open gate, which `draining` opens for good.
    pub(crate) fn new(draining: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            shut_until: Mutex::new(None),
            draining,
        })
    }

    /// The throttle refused an attempt, and its window ends in `ends_in`: fetch
    /// nothing more until then. A later end than the one already set wins.
    pub(crate) fn shut_for(&self, ends_in: Duration) {
        let until = Instant::now() + ends_in;
        let mut shut = self
            .shut_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if shut.is_none_or(|current| current < until) {
            *shut = Some(until);
        }
    }

    /// When the gate opens, if it is shut now.
    fn shut(&self) -> Option<Instant> {
        if self.draining.is_cancelled() {
            return None;
        }
        let mut shut = self
            .shut_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match *shut {
            Some(until) if until > Instant::now() => Some(until),
            Some(_) => {
                *shut = None;
                None
            }
            None => None,
        }
    }

    /// The layer that holds a worker's readiness to this gate.
    pub(crate) fn layer(self: &Arc<Self>) -> ThrottleGateLayer {
        ThrottleGateLayer {
            gate: Arc::clone(self),
        }
    }
}

/// Holds a worker's service unready while its [`ThrottleGate`] is shut. The
/// outermost layer, so no permit is taken while it is.
pub(crate) struct ThrottleGateLayer {
    gate: Arc<ThrottleGate>,
}

impl<S> Layer<S> for ThrottleGateLayer {
    type Service = Gated<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Gated {
            inner,
            gate: Arc::clone(&self.gate),
            opens: None,
            drain: Box::pin(self.gate.draining.clone().cancelled_owned()),
        }
    }
}

/// A service ready only while its gate is open.
pub(crate) struct Gated<S> {
    inner: S,
    gate: Arc<ThrottleGate>,
    /// Wakes the worker when the gate opens.
    opens: Option<Pin<Box<Sleep>>>,
    /// Wakes the worker when the drain opens the gate.
    drain: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl<S, R> Service<R> for Gated<S>
where
    S: Service<R>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if let Some(until) = self.gate.shut() {
            let opens = self
                .opens
                .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(until)));
            if opens.deadline() != until {
                opens.as_mut().reset(until);
            }
            let opened = opens.as_mut().poll(cx).is_ready();
            let drained = self.drain.as_mut().poll(cx).is_ready();
            if !opened && !drained {
                return Poll::Pending;
            }
        }
        self.opens = None;
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: R) -> Self::Future {
        self.inner.call(request)
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use super::*;

    /// A service that is always ready, counting how often it was asked.
    #[derive(Default)]
    struct Ready {
        asked: usize,
    }

    impl Service<()> for Ready {
        type Response = ();
        type Error = std::convert::Infallible;
        type Future = std::future::Ready<Result<(), Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.asked += 1;
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, (): ()) -> Self::Future {
            std::future::ready(Ok(()))
        }
    }

    fn polled<S: Service<()>>(service: &mut S) -> bool {
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        service.poll_ready(&mut cx).is_ready()
    }

    /// Open, the gate asks the service it wraps; shut, it is not ready and asks
    /// nothing — no permit is taken — until the window ends, when it wakes the
    /// worker on its own.
    #[tokio::test]
    async fn a_shut_gate_holds_the_worker_unready_until_the_window_ends() {
        let gate = ThrottleGate::new(CancellationToken::new());
        let mut gated = gate.layer().layer(Ready::default());
        assert!(polled(&mut gated), "an open gate is ready");
        assert_eq!(gated.inner.asked, 1);

        let started = Instant::now();
        gate.shut_for(Duration::from_millis(300));
        assert!(!polled(&mut gated), "a shut gate is not ready");
        assert_eq!(gated.inner.asked, 1, "and asks the service nothing");

        gate.shut_for(Duration::from_millis(100));
        poll_fn(|cx| gated.poll_ready(cx))
            .await
            .expect("ready once the window ends");
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(300) && waited < Duration::from_secs(2),
            "at the later of the two ends, never the earlier: {waited:?}",
        );
        assert!(polled(&mut gated), "and open from then on");
    }

    /// The drain opens a shut gate at once, so jobs waiting in the replica are
    /// handed back rather than held past its stop.
    #[tokio::test]
    async fn the_drain_opens_a_shut_gate() {
        let draining = CancellationToken::new();
        let gate = ThrottleGate::new(draining.clone());
        let mut gated = gate.layer().layer(Ready::default());
        gate.shut_for(Duration::from_secs(3600));
        assert!(!polled(&mut gated));

        let opened = tokio::spawn(async move { poll_fn(|cx| gated.poll_ready(cx)).await.is_ok() });
        tokio::time::sleep(Duration::from_millis(10)).await;
        draining.cancel();
        assert!(opened.await.expect("the wait ends"), "ready once draining");
    }
}

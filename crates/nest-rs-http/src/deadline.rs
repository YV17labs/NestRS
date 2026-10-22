//! [`RequestDeadline`] — when a request must be answered by, handed down in its
//! extensions to the unit of work it runs, which cuts its body there and never
//! its commit.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use http::{HeaderValue, StatusCode, header};
use tokio::time::Instant;

use crate::response::Response;

/// When a request must be answered by — its arrival plus the request timeout —
/// and whether a unit of work under it took the cut over.
///
/// The edge puts it in the request's extensions whenever a request timeout is
/// set. The layer running the request's unit of work takes it over: it cuts
/// its handler at [`at`](Self::at), answering [`timed_out`](Self::timed_out),
/// and hands back its driver's settle net, which the edge then waits past the
/// deadline — a net that fires only on a unit that did not settle, answered
/// `500` without `Retry-After`, since what it wrote may have landed.
#[derive(Clone, Debug)]
pub struct RequestDeadline(Arc<Deadline>);

#[derive(Debug)]
struct Deadline {
    at: Instant,
    timeout: Duration,
    /// The net past the deadline a unit took the cut over with.
    unit_net: OnceLock<Duration>,
}

impl RequestDeadline {
    /// The deadline `timeout` past a request's arrival, at `at`.
    pub fn new(at: Instant, timeout: Duration) -> Self {
        Self(Arc::new(Deadline {
            at,
            timeout,
            unit_net: OnceLock::new(),
        }))
    }

    /// The instant the request must be answered by.
    pub fn at(&self) -> Instant {
        self.0.at
    }

    /// The request timeout the deadline was set from: the budget a cut
    /// reports.
    pub fn timeout(&self) -> Duration {
        self.0.timeout
    }

    /// A unit of work takes the cut over: it cuts its body at [`at`](Self::at)
    /// and settles within `settle_net` of it, which the edge then waits.
    pub fn hand_to_unit(&self, settle_net: Duration) {
        // A second unit under one request takes the first one's net.
        let _first = self.0.unit_net.set(settle_net);
    }

    /// The net past the deadline a unit took the cut over with, if one did.
    pub fn unit_net(&self) -> Option<Duration> {
        self.0.unit_net.get().copied()
    }

    /// What a request whose body was cut at the deadline is answered — `503`,
    /// honest since nothing it ran committed — said once at `warn`.
    pub fn timed_out(&self) -> Response {
        let timeout = self.0.timeout;
        tracing::warn!(target: crate::target::HTTP, ?timeout, "request timed out");
        timed_out(timeout)
    }
}

/// What a request that outran its budget is answered with: `503`, not `504`, since
/// RFC 9110 §15.6.5 scopes `504` to a gateway and this server is the origin (§15.6.4).
pub(crate) fn timed_out(timeout: Duration) -> Response {
    let seconds = nest_rs_core::__private::retry_after_secs(timeout);
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::RETRY_AFTER, HeaderValue::from(seconds))
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retry_after_a_timeout_states_is_the_budget_that_ran_out() {
        let value = |secs: u64, nanos: u32| {
            timed_out(Duration::new(secs, nanos))
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        assert_eq!(value(30, 0).as_deref(), Some("30"));
        assert_eq!(value(0, 500_000_000).as_deref(), Some("1"), "never zero");
        assert_eq!(
            value(30, 1).as_deref(),
            Some("31"),
            "rounded up — a client that waits less may collide with the attempt",
        );
    }

    #[test]
    fn a_unit_layer_answers_the_edge_s_timeout_and_says_it_once() {
        let logs = nest_rs_testing::LogCapture::install();
        let deadline = RequestDeadline::new(Instant::now(), Duration::from_secs(20));
        let answered = deadline.timed_out();
        assert_eq!(answered.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            answered.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from_static("20"))
        );
        logs.expect_one(crate::target::HTTP, "request timed out");
    }

    #[test]
    fn the_first_unit_to_take_the_deadline_over_sets_its_net() {
        let deadline = RequestDeadline::new(Instant::now(), Duration::from_secs(20));
        assert_eq!(deadline.unit_net(), None);
        deadline.hand_to_unit(Duration::from_secs(16));
        deadline.clone().hand_to_unit(Duration::from_secs(31));
        assert_eq!(deadline.unit_net(), Some(Duration::from_secs(16)));
    }
}

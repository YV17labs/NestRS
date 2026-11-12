//! The canonical name of the unit of work this edge opens.
//!
//! [`operation_span!`](nest_rs_core::operation_span) and
//! [`operation_line!`](nest_rs_core::operation_line) take it as a path, and
//! refuse one this crate did not declare:
//!
//! ```
//! use nest_rs_core::operation_log::OK;
//! use nest_rs_core::{Correlation, operation_line, operation_span};
//!
//! let correlation = Correlation::minted(None);
//! let started = std::time::Instant::now();
//! let span = operation_span!(nest_rs_http::unit::REQUEST, &correlation, http.request.method = "GET");
//! operation_line!(
//!     nest_rs_http::unit::REQUEST,
//!     span: &span,
//!     outcome: OK,
//!     started: started,
//!     verb = "GET",
//! );
//! assert_eq!(nest_rs_http::unit::REQUEST.name(), "http.request");
//! ```

use nest_rs_core::operation_log::{MetricAttribute, Unit, UnitDuration};

/// One HTTP request, filed once the response body has been written.
pub const REQUEST: Unit = nest_rs_core::unit!(
    "http.request",
    target: crate::target::HTTP,
    kind: Server,
    duration: REQUEST_DURATION,
);

/// The HTTP conventions' server duration, keyed by what the span says in the
/// server's words: the known method, the route template, never the path.
const REQUEST_DURATION: UnitDuration = UnitDuration::new(
    "http.server.request.duration",
    "Duration of HTTP server requests.",
    &[
        MetricAttribute::bounded("http.request.method"),
        MetricAttribute::bounded("url.scheme"),
        MetricAttribute::bounded("http.route"),
        MetricAttribute::bounded("http.response.status_code"),
        MetricAttribute::bounded("error.type"),
    ],
);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(REQUEST.edge(), Some(Edge::Http));
    }
}

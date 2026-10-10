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
//!     method = "GET",
//! );
//! assert_eq!(nest_rs_http::unit::REQUEST.name(), "http.request");
//! ```

use nest_rs_core::operation_log::Unit;

/// One HTTP request, filed once the response body has been written.
pub const REQUEST: Unit =
    nest_rs_core::unit!("http.request", target: crate::target::HTTP, kind: Server);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(REQUEST.edge(), Some(Edge::Http));
    }
}

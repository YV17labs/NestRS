//! The canonical name of the unit of work this edge opens.
//!
//! Declared here rather than in the kernel, through [`nest_rs_core::unit!`],
//! whose compile-time evaluation holds the `<edge>.<unit>` grammar: both are
//! argued once, in [`nest_rs_core::operation_log`].
//!
//! One constant, three slots: the [`operation_span!`](nest_rs_core::operation_span)
//! that opens the unit, and the [`operation_line!`](nest_rs_core::operation_line)
//! that files its line, as the line's `name:` and its `message`. Both take the
//! unit as a path, and both refuse one this crate did not declare:
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

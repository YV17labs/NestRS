use std::time::Instant;

use nest_rs::core::Correlation;
use nest_rs::core::operation_log::OK;
use nest_rs::core::tracing::Span;

use crate::WORK;

/// Open [`WORK`]'s span under `correlation`, as an edge opens its unit's.
pub fn open(correlation: &Correlation) -> Span {
    nest_rs::core::operation_span!(WORK, correlation)
}

/// File [`WORK`]'s operation line for a unit opened at `started`.
pub fn settle(span: &Span, started: Instant) {
    nest_rs::core::operation_line!(WORK, span: span, outcome: OK, started: started);
}

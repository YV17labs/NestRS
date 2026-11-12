use std::time::Instant;

use nest_rs::core::Correlation;
use nest_rs::core::operation_log::OK;
use nest_rs::core::tracing::Span;
use nest_rs::core::tracing::field::Empty;

use crate::{TIMED, WORK};

/// Open [`WORK`]'s span under `correlation`, as an edge opens its unit's.
pub fn open(correlation: &Correlation) -> Span {
    nest_rs::core::operation_span!(WORK, correlation)
}

/// Open [`TIMED`]'s span under `correlation`, for `step`, as `verb` asked it;
/// its code is recorded once the work answers.
pub fn open_timed(correlation: &Correlation, step: &'static str, verb: &str) -> Span {
    nest_rs::core::operation_span!(
        TIMED,
        correlation,
        probe.step = step,
        probe.verb = verb,
        probe.code = Empty,
    )
}

/// File [`WORK`]'s operation line for a unit opened at `started`.
pub fn settle(span: &Span, started: Instant) {
    nest_rs::core::operation_line!(WORK, span: span, outcome: OK, started: started);
}

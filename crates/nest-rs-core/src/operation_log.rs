//! One line per unit of work, and the vocabulary every edge files it with.
//!
//! The ids on a log line relate lines to each other; they do not say what the
//! work *was*. A line rendered by
//! [`TextFormat`](crate::logging::TextFormat) carries no span state at all — by
//! design, since a span's attributes are not part of a log record — so the
//! identity of a unit of work has to arrive as **event** attributes, on a line
//! the edge emits once per unit. HTTP has always had one; the other edges did
//! not, and their work was anonymous on the console as a result.
//!
//! What each edge writes is its own — a route, an event name, a job id, a tool
//! name — but the target, the duration formula and the outcome words are shared,
//! so an operator queries the family one way and a new edge cannot invent a
//! fourth word for "it failed".
//!
//! **Field names on a line are flat, never dotted**, and that is not a style
//! choice: `tracing`'s macro grammar is locally ambiguous on a dotted name when
//! the target is a path expression rather than a literal, and [`TARGET`] is a
//! path — so `ws.event = %event` is a compile error on the line while the
//! identical field compiles on the span beside it. The span keeps the dotted,
//! conventions-shaped names for the export; the line uses the edge's short
//! ones.

use std::time::Instant;

/// The target every operation line is emitted on, whichever edge files it.
///
/// **One target is the family's toggle.** `<PREFIX>_LOG=info,nest_rs::access=off`
/// silences every edge's line at once, which is why no edge grows a `#[config]`
/// field — and the `for_root` seam that one would oblige — to hold a boolean the
/// filter already answers. `NESTRS_HTTP__ACCESS_LOG` predates the family and
/// stays: it is an app's pinned config rather than a deployment's filter.
pub const TARGET: &str = "nest_rs::access";

/// The unit of work completed as asked.
pub const OK: &str = "ok";
/// It returned an error, or was refused. The edge's own fields say which.
pub const ERROR: &str = "error";
/// Developer code unwound. Distinct from [`ERROR`] because the two are read
/// differently under incident: one is a handled path, the other is not.
pub const PANIC: &str = "panic";

/// Elapsed milliseconds at microsecond resolution.
///
/// Rendered in milliseconds because that is the unit an operator compares
/// against a timeout, and measured to the microsecond because a sub-millisecond
/// unit of work is the common case and `0` tells them nothing.
pub fn duration_ms(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_duration_keeps_microseconds_rather_than_rounding_to_zero() {
        // The shape that matters: a fast unit of work reports a fraction, not
        // the `0` an integer-millisecond formula would give it.
        let start = Instant::now();
        let elapsed = duration_ms(start);
        assert!(elapsed >= 0.0, "{elapsed}");
        assert!(elapsed < 1000.0, "a no-op cannot take a second: {elapsed}");
        assert_eq!(
            elapsed,
            (elapsed * 1000.0).round() / 1000.0,
            "resolution is the microsecond",
        );
    }

    #[test]
    fn the_outcome_words_are_distinct() {
        // A new edge picking a fourth word is what this vocabulary exists to
        // prevent; that they differ from each other is the cheap half of it.
        assert_ne!(OK, ERROR);
        assert_ne!(ERROR, PANIC);
    }
}

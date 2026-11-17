//! A package's own units of work, for `nest-rs-opentelemetry`'s suite: declared
//! under the crate's name, opened with the framework's operation span, and
//! filed with its operation line — one of them timed.

mod unit;
mod work;

/// This crate's span target.
pub const TARGET: &str = "nestrs_unit_probe";

pub use unit::{CODE, STEP, TIMED, TIMED_DURATION, VERB, WORK};
pub use work::{open, open_timed, settle};

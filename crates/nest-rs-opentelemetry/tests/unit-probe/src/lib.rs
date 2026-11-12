//! A package's own unit of work, for `nest-rs-opentelemetry`'s suite: declared
//! under the crate's name, opened with the framework's operation span, and
//! filed with its operation line.

mod unit;
mod work;

/// This crate's span target.
pub const TARGET: &str = "nestrs_unit_probe";

pub use unit::WORK;
pub use work::{open, settle};

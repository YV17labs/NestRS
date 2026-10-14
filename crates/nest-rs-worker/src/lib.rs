//! The bound every worker job runs within: [`JOB_TIMEOUT`], and the
//! [`JobTimedOut`] failure an attempt cut at it reports. A job's unit of work
//! runs through `nest_rs_database::run_unit`, like every other edge's.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod constants;
mod error;

pub use constants::JOB_TIMEOUT;
pub use error::JobTimedOut;

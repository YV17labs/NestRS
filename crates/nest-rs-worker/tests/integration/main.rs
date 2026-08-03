//! Integration suite root for `nest-rs-worker`. Every test lives in the
//! module named for the `src/` concern it covers: [`context`] for the
//! [`nest_rs_worker::JobContext`] contract through `run_in_job_context`.

mod context;

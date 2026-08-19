//! The canonical name of the unit of work this edge opens.
//!
//! Declared by the **contract** crate and read by whichever backend runs the
//! job — owns, not emits, the same split `nest_rs_core::target` states for a
//! span target: a concern several crates emit on is named once by the crate
//! the others already depend on. The grammar — `<edge>.<unit>`, lowercase, one
//! dot, the namespace from the closed edge vocabulary — belongs to
//! `nest_rs_core::operation_log`, and the `units` join in `nest-rs-conformance`
//! holds every edge to it.

/// One queue job attempt.
pub const JOB: &str = "queue.job";

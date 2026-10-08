//! The canonical name of the unit of work this edge opens — declared by the
//! port, whose [`QueueWorker`](crate::QueueWorker) opens it on every backend.

use nest_rs_core::operation_log::Unit;

/// One queue job attempt.
pub const JOB: Unit = nest_rs_core::unit!("queue.job", target: crate::TARGET, kind: Consumer);

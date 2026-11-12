//! The queue binding's own failures, each handed to the port as the opaque
//! source of a [`QueueError::Backend`](nest_rs_queue::QueueError::Backend).

use thiserror::Error;

/// A reply the queue's scripts and commands never send — a Redis that is not
/// the one the scripts ran on, or a defect here. Names the call and the shape it
/// expected, never what came back, which may carry a record.
#[derive(Debug, Error)]
#[error("Redis answered {call} in a shape it never sends: expected {expected}")]
pub(crate) struct UnexpectedReply {
    pub(crate) call: &'static str,
    pub(crate) expected: &'static str,
}

/// A checkpoint written by a delivery this worker no longer holds: its lease
/// lapsed and another delivery of the job holds it, so its state is that
/// delivery's to write.
#[derive(Debug, Error)]
#[error("the delivery writing this checkpoint no longer holds its job; another delivery does")]
pub(crate) struct CheckpointFenced;

/// A disposition the port added after this backend was written: the port
/// sends one only to a backend declaring the capability that names it, so
/// meeting one is a defect of the port or of this declaration.
#[derive(Debug, Error)]
#[error("the queue port ended a delivery in a way the Redis backend does not declare")]
pub(crate) struct UnknownDisposition;

/// A consumer prepared a second time: each worker builds its own, so a second
/// `prepare` is two workers sharing one consumer name in every group.
#[derive(Debug, Error)]
#[error("the Redis queue consumer was prepared twice; each queue worker binds its own")]
pub(crate) struct PreparedTwice;

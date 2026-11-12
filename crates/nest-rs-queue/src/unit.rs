//! The canonical name of the unit of work this edge opens — declared by the
//! port, whose [`QueueWorker`](crate::QueueWorker) opens it on every backend.

use nest_rs_core::operation_log::{MetricAttribute, Unit, UnitDuration};

/// One queue job attempt.
pub const JOB: Unit = nest_rs_core::unit!(
    "queue.job",
    target: crate::TARGET,
    kind: Consumer,
    duration: JOB_DURATION,
);

/// The messaging conventions' processing duration, keyed by the backend and
/// the queue a processor declares.
const JOB_DURATION: UnitDuration = UnitDuration::new(
    "messaging.process.duration",
    "Duration of processing operation.",
    &[
        MetricAttribute::bounded("messaging.system"),
        MetricAttribute::bounded("messaging.operation.name"),
        MetricAttribute::bounded("messaging.destination.name"),
        MetricAttribute::bounded("error.type"),
    ],
);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(JOB.edge(), Some(Edge::Queue));
    }
}

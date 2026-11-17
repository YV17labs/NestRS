use nest_rs::core::duration_histogram::{DurationHistogram, MetricAttribute, REQUEST_BOUNDARIES};
use nest_rs::core::operation_log::Unit;

/// One piece of the probe's work.
pub const WORK: Unit = nest_rs::core::unit!(
    "nestrs_unit_probe.work",
    target: crate::TARGET,
    kind: Internal
);

/// One piece of the probe's work whose duration is recorded.
pub const TIMED: Unit = nest_rs::core::unit!(
    "nestrs_unit_probe.timed",
    target: crate::TARGET,
    kind: Internal,
    duration: TIMED_DURATION,
);

/// What [`TIMED`] records its duration in: a field the probe bounds, one read
/// through a known set, and one recorded once the work answers.
pub const TIMED_DURATION: DurationHistogram = DurationHistogram::new(
    "nestrs_unit_probe.timed.duration",
    "Duration of the probe's timed work.",
    REQUEST_BOUNDARIES,
    &[
        MetricAttribute::bounded(STEP),
        MetricAttribute::known(VERB, &["read", "write"]),
        MetricAttribute::bounded(CODE),
    ],
);

/// The field naming the step, which the probe bounds.
pub const STEP: &str = "probe.step";

/// The field naming the verb, as a caller sent it.
pub const VERB: &str = "probe.verb";

/// The field recording the code the work answered, once it has.
pub const CODE: &str = "probe.code";

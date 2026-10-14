use nest_rs::core::operation_log::Unit;

/// One piece of the probe's work.
pub const WORK: Unit = nest_rs::core::unit!(
    "nestrs_unit_probe.work",
    target: crate::TARGET,
    kind: Internal
);

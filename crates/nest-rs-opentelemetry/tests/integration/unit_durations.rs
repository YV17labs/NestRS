//! Covers `src/unit_durations.rs` as `init` installs it: a unit declaring a
//! duration records in it whatever the console writes, and the export carries
//! it.

use nest_rs_core::Correlation;
use nestrs_unit_probe::TIMED_DURATION;

use crate::otlp::{Collector, metric_exports, trusting};

#[test]
fn a_units_duration_is_exported_with_the_console_filtered_to_warn() {
    let collector = Collector::serving();
    let otel = trusting(
        &collector,
        &[
            ("RUST_LOG", "warn"),
            ("OTEL_TRACES_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );

    drop(nestrs_unit_probe::open_timed(
        &Correlation::minted(None),
        "fetch",
        "read",
    ));
    drop(otel);

    let exports = metric_exports(&collector);
    assert!(
        exports
            .iter()
            .any(|export| export.holds(TIMED_DURATION.name())),
        "the duration reached the collector: {exports:?}",
    );
}

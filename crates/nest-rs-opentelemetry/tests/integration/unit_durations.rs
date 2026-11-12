//! Covers `src/unit_durations.rs` as `init` installs it: a unit declaring a
//! duration records in it whatever the console writes, and the export carries
//! it.

use nest_rs_core::Correlation;
use nestrs_unit_probe::TIMED_DURATION;

use crate::otlp::{Collector, trusting};

/// Whether `body` holds `needle`: an OTLP/protobuf export writes a metric's
/// name as its bytes.
pub(crate) fn holds(body: &[u8], needle: &[u8]) -> bool {
    body.windows(needle.len()).any(|window| window == needle)
}

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

    let exports: Vec<_> = collector
        .received()
        .into_iter()
        .filter(|export| export.path == "/v1/metrics")
        .collect();
    assert!(
        exports
            .iter()
            .any(|export| holds(&export.body, TIMED_DURATION.name().as_bytes())),
        "the duration reached the collector: {exports:?}",
    );
}

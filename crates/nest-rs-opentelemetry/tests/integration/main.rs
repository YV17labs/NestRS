//! `OpenTelemetryModule` must not be imported without `OpenTelemetry::init` first — that would
//! register no-op telemetry providers and drop traces/metrics silently, so it
//! panics at boot instead. nextest runs every test in a process of its own, so a
//! test that initialises OpenTelemetry never does it for another.

mod module;

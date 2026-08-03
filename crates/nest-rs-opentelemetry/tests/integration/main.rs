//! `OpenTelemetryModule` must not be imported without `OpenTelemetry::init` first — that would
//! register no-op telemetry providers and drop traces/metrics silently, so it
//! panics at boot instead. This runs as its own test binary so no sibling test
//! initialises OpenTelemetry and sets the global flag.

mod module;

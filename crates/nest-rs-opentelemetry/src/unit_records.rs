//! The records a unit files, named by the unit's trace and span where the
//! OpenTelemetry context names no span: the SDK reads only the context the
//! trace layer activates (`SdkLogger::emit`), which no unit has where traces do
//! not export, or where the console's filter closed the unit's span.

use std::time::Duration;

use opentelemetry::InstrumentationScope;
use opentelemetry::logs::LogRecord as _;
use opentelemetry::trace::{SpanId, TraceFlags, TraceId};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogProcessor, SdkLogRecord};

/// Names a record the SDK found no active span for by the unit it was filed
/// in, as the console's line names it; laid ahead of the batch processor, which
/// exports the record as this hands it on.
#[derive(Debug)]
pub(crate) struct UnitRecords;

impl LogProcessor for UnitRecords {
    fn emit(&self, record: &mut SdkLogRecord, _: &InstrumentationScope) {
        if record.trace_context().is_some() {
            return;
        }
        if let Some(unit) = nest_rs_core::current_traceparent() {
            record.set_trace_context(
                TraceId::from_bytes(unit.trace_id.to_bytes()),
                SpanId::from_bytes(unit.parent_id.to_bytes()),
                Some(TraceFlags::new(unit.flags.bits())),
            );
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    /// Holds nothing to release: the SDK's default would say otherwise at `warn`.
    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }
}

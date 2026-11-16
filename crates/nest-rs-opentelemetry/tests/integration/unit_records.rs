//! Covers `src/unit_records.rs`: a line filed inside a unit is exported naming
//! the unit's trace and span where no span is active for the SDK — traces not
//! exported, whatever the console's filter makes of the unit's span, or a
//! request's line, filed as its answer ends — and naming the span the SDK
//! found active where one is.

use nest_rs_core::{Correlation, RequestContinuation, module};
use nest_rs_http::{HttpConfig, HttpModule, HttpSetup};
use nest_rs_testing::TestApp;
use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use prost::Message as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::otlp::{Collector, log_exports, trusting};

/// The body of the line each test files inside its unit.
const LINE: &str = "a line filed in the unit";

/// The logs exported, the traces and the metrics not.
const LOGS_ALONE: [(&str, &str); 2] = [
    ("OTEL_TRACES_EXPORTER", "none"),
    ("OTEL_METRICS_EXPORTER", "none"),
];

/// The record `collector` received for the line whose body is `line`.
fn record_of(collector: &Collector, line: &str) -> LogRecord {
    let exports = log_exports(collector);
    exports
        .iter()
        .flat_map(|export| {
            ExportLogsServiceRequest::decode(export.body.as_slice())
                .expect("an OTLP/protobuf log export")
                .resource_logs
        })
        .flat_map(|resource| resource.scope_logs)
        .flat_map(|scope| scope.log_records)
        .find(|record| {
            matches!(
                record.body.as_ref().and_then(|body| body.value.as_ref()),
                Some(Value::StringValue(body)) if body == line
            )
        })
        .unwrap_or_else(|| panic!("the line was exported: {exports:?}"))
}

/// The unit's correlation, what `file` returned inside it, and the record of
/// the line it filed, with `vars` over an export to a collector.
fn filed_in_a_unit<T>(
    vars: &[(&str, &str)],
    file: impl FnOnce() -> T,
) -> (Correlation, T, LogRecord) {
    let collector = Collector::serving();
    let otel = trusting(&collector, vars);

    let correlation = Correlation::minted(None);
    let unit = nestrs_unit_probe::open(&correlation);
    let returned =
        RequestContinuation::new(None, correlation.clone()).enter(|| unit.in_scope(file));
    drop(unit);
    drop(otel);

    (correlation, returned, record_of(&collector, LINE))
}

/// The record names the unit it was filed in, as the console's line does.
fn assert_names_the_unit(correlation: &Correlation, record: &LogRecord) {
    assert_eq!(
        record.trace_id,
        correlation.trace_id().to_bytes(),
        "the record carries the unit's trace id"
    );
    assert_eq!(
        record.span_id,
        correlation.span_id().to_bytes(),
        "the record carries the unit's span id"
    );
}

#[test]
fn with_traces_not_exported_a_line_in_a_unit_is_exported_with_the_units_ids() {
    let (correlation, (), record) =
        filed_in_a_unit(&LOGS_ALONE, || tracing::info!(target: "probe", "{LINE}"));
    assert_names_the_unit(&correlation, &record);
}

/// The console's filter decides what is written, never which unit a written
/// line names: a `warn` line under `RUST_LOG=warn`, whose unit's `info` span
/// the filter closes, still carries the unit's ids.
#[test]
fn with_traces_not_exported_and_the_console_quieted_a_line_in_a_unit_is_exported_with_the_units_ids()
 {
    let vars = [LOGS_ALONE[0], LOGS_ALONE[1], ("RUST_LOG", "warn")];
    let (correlation, (), record) = filed_in_a_unit(&vars, || {
        tracing::warn!(target: "probe", "{LINE}");
    });
    assert_names_the_unit(&correlation, &record);
}

/// Where traces export, a line filed in a span an app opened inside the unit
/// names that span, the one the SDK found active, as the log data model asks.
#[test]
fn with_traces_exported_a_line_in_an_apps_span_is_exported_naming_that_span() {
    let (correlation, apps_span, record) =
        filed_in_a_unit(&[("OTEL_METRICS_EXPORTER", "none")], || {
            let span = tracing::info_span!(target: "probe", "probe.step");
            span.in_scope(|| tracing::info!(target: "probe", "{LINE}"));
            span.context().span().span_context().span_id()
        });

    assert_eq!(record.trace_id, correlation.trace_id().to_bytes());
    assert_ne!(
        apps_span.to_bytes(),
        correlation.span_id().to_bytes(),
        "the app's span is a child of the unit's"
    );
    assert_eq!(
        record.span_id,
        apps_span.to_bytes(),
        "the record names the app's span"
    );
}

/// What a trusted peer sends, continued by a request from loopback.
const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

fn behind_loopback() -> HttpSetup {
    HttpModule::for_root(HttpConfig {
        trusted_proxies: vec!["127.0.0.1".parse().expect("a proxy entry")],
        ..HttpConfig::default()
    })
}

#[module(imports = [behind_loopback()])]
struct Served;

/// The `traceresponse` a request continuing [`PARENT`] was answered with, and
/// the record of the line it filed as its answer ended — a `404`'s, at
/// `info` — with `vars` over an export to a collector.
fn request_line_exported(vars: &[(&str, &str)]) -> (String, LogRecord) {
    let collector = Collector::serving();
    let otel = trusting(&collector, vars);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let traceresponse = runtime.block_on(async {
        let app = TestApp::for_module::<Served>()
            .await
            .expect("the app boots");
        let response = app
            .http()
            .get("/nowhere")
            .peer_addr("127.0.0.1:40000".parse().expect("a socket address"))
            .header("traceparent", PARENT)
            .send()
            .await;
        response.assert_status(nest_rs_http::StatusCode::NOT_FOUND);
        response.0.headers()["traceresponse"]
            .to_str()
            .expect("a visible header")
            .to_owned()
    });
    drop(runtime);
    drop(otel);
    (
        traceresponse,
        record_of(&collector, nest_rs_http::unit::REQUEST.name()),
    )
}

/// The record names the trace and the span `traceresponse` says the request
/// ran under.
fn assert_names_the_request(traceresponse: &str, record: &LogRecord) {
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|byte| format!("{byte:02x}")).collect() };
    let named = format!("00-{}-{}-", hex(&record.trace_id), hex(&record.span_id));
    assert!(
        traceresponse.starts_with(&named),
        "the record names {named}, the request ran under {traceresponse}"
    );
}

/// A request's line is filed as its answer ends, past the span its handler ran
/// in: no span is active for the SDK there, traces exported or not.
#[test]
fn with_traces_exported_a_requests_line_is_exported_naming_the_request() {
    let (traceresponse, record) = request_line_exported(&[("OTEL_METRICS_EXPORTER", "none")]);
    assert_names_the_request(&traceresponse, &record);
}

#[test]
fn with_traces_not_exported_a_requests_line_is_exported_naming_the_request() {
    let (traceresponse, record) = request_line_exported(&LOGS_ALONE);
    assert_names_the_request(&traceresponse, &record);
}

//! Baseline console logging, installed at boot when no global `tracing`
//! subscriber is set; an observability stack installed in `main` first is left
//! in place. Configuration is env-only:
//!
//! - `<PREFIX>_LOG` (falling back to `RUST_LOG`) — `EnvFilter` directives,
//!   default `info`. Set-but-unparseable aborts boot.
//! - `<PREFIX>_LOG_FORMAT` — `text` or `json`; defaults by build profile
//!   (text in debug, JSON in release), unrecognized values keep the default.
//! - `<PREFIX>_LOG_SOURCE_LOCATION` — append the emitting `file:line` to each
//!   event; off by default (it leaks source paths).
//!
//! The same variables drive the console layer of `nest-rs-opentelemetry`.
//! Behind the default `logging` cargo feature.

use std::borrow::Cow;
use std::fmt::{self, Write as _};

use anyhow::Result;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::field::VisitOutput;
use tracing_subscriber::fmt::format::{DefaultVisitor, JsonFields, Writer};
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

use crate::env_prefix::EnvPrefix;
use crate::line_safe::{LineSafe, forges};
use crate::request_scope::current_request_ctx;
use crate::trace_context::{Correlation, field};

/// The tails of the framework-wide logging variables, joined with the
/// deployment's prefix by [`EnvPrefix::var`](crate::EnvPrefix::var).
///
/// `nest-rs-cli` spells them again in its templates: it links no `nest-rs-*`
/// crate.
pub mod var {
    /// `<PREFIX>_LOG` — the `EnvFilter` directive the process logs under.
    pub const FILTER: &str = "LOG";
    /// `<PREFIX>_LOG_FORMAT` — `text` or `json`.
    pub const FORMAT: &str = "LOG_FORMAT";
    /// `<PREFIX>_LOG_SOURCE_LOCATION` — whether a line carries file and line.
    pub const SOURCE_LOCATION: &str = "LOG_SOURCE_LOCATION";
}

/// Shape of the console log layer's output — the grammar of
/// `<PREFIX>_LOG_FORMAT`, for every subscriber the framework ships.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable text; the default in debug builds.
    #[default]
    Text,
    /// One JSON object per event; the default in release builds.
    Json,
}

impl LogFormat {
    /// `text`/`json`, trimmed and case-insensitive; `None` for anything else, so
    /// a caller decides whether an unrecognized value is a default or an error.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    /// Text in debug, JSON in release.
    pub fn by_profile() -> Self {
        if cfg!(debug_assertions) {
            Self::Text
        } else {
            Self::Json
        }
    }

    /// [`parse`](Self::parse) falling back to [`by_profile`](Self::by_profile).
    pub fn resolve(raw: Option<&str>) -> Self {
        raw.and_then(Self::parse).unwrap_or_else(Self::by_profile)
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the logging bootstrap runs before any config exists"
)]
fn bool_from_env(name: &str) -> bool {
    std::env::var(name).ok().and_then(|v| crate::parse_bool(&v)) == Some(true)
}

/// SGR sequences spelled out: tracing-subscriber's own `Style` is private.
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";

/// The colour and five-column label `Format<Full>` gives each level, byte for
/// byte.
fn level_style(level: &Level) -> (&'static str, &'static str) {
    match *level {
        Level::TRACE => ("\x1b[35m", "TRACE"),
        Level::DEBUG => ("\x1b[34m", "DEBUG"),
        Level::INFO => ("\x1b[32m", " INFO"),
        Level::WARN => ("\x1b[33m", " WARN"),
        Level::ERROR => ("\x1b[31m", "ERROR"),
    }
}

/// An unreadable clock writes a placeholder rather than failing the line, as
/// `Format<Full>` does.
fn write_timestamp(writer: &mut Writer<'_>) -> fmt::Result {
    if SystemTime.format_time(writer).is_err() {
        writer.write_str("<unknown time>")?;
    }
    Ok(())
}

fn dimmed(
    writer: &mut Writer<'_>,
    ansi: bool,
    body: impl FnOnce(&mut Writer<'_>) -> fmt::Result,
) -> fmt::Result {
    if ansi {
        writer.write_str(ANSI_DIM)?;
        body(writer)?;
        writer.write_str(ANSI_RESET)
    } else {
        body(writer)
    }
}

/// Run `write` against the current correlation, borrowed: `current_actor_id`
/// would allocate a `String` per event.
fn with_current_correlation(write: impl FnOnce(&Correlation) -> fmt::Result) -> fmt::Result {
    current_request_ctx(|ctx| write(&ctx.correlation)).unwrap_or(Ok(()))
}

/// Render an event's own fields, `duration_ms` padded to
/// [`DURATION_DECIMALS`](crate::operation_log::DURATION_DECIMALS) so it reads as
/// a column; JSON does not pad.
fn write_event_fields(writer: &mut Writer<'_>, event: &Event<'_>) -> fmt::Result {
    let mut visitor = FixedWidthDurations(DefaultVisitor::new(writer.by_ref(), true));
    event.record(&mut visitor);
    visitor.0.finish()
}

struct FixedWidthDurations<'a>(DefaultVisitor<'a>);

impl Visit for FixedWidthDurations<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == crate::operation_log::DURATION_MS {
            let decimals = crate::operation_log::DURATION_DECIMALS;
            // `Arguments` debug-prints as it displays, so no quotes are added.
            self.0
                .record_debug(field, &format_args!("{value:.decimals$}"));
        } else {
            self.0.record_f64(field, value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        // `DefaultVisitor` writes the message bare, bypassing `record_debug`.
        if field.name() == "message" {
            self.0
                .record_debug(field, &format_args!("{}", LineSafe(&value)));
        } else {
            self.0.record_str(field, value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let message = crate::error_message(value);
        self.0
            .record_debug(field, &format_args!("{}", LineSafe(&message)));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.record_debug(field, &LineSafe(&value));
    }
}

/// The console format for **text** output.
///
/// A line is `timestamp level target: message fields`, then the **W3C Trace
/// Context of the unit of work it belongs to** — `trace_id`, `span_id`, and
/// `actor_id` once a guard resolved a principal:
///
/// ```text
/// 2026-08-18T14:49:16.529159Z DEBUG fixture::lane: creating post title="hello" trace_id=01a014ec214e7163844619af2aeaeca4 span_id=b0c095c368ba3d72
/// ```
///
/// The ids are read from the ambient context, and a line carries no span
/// attributes or names.
///
/// The builder's `with_file`, `with_line_number`, `with_timer`, `with_target`,
/// `with_level` and `with_thread_ids` do not reach the output: `file:line` is
/// [`source_location`](Self::new), and ANSI follows the writer.
#[derive(Clone, Copy, Debug)]
pub struct TextFormat {
    source_location: bool,
}

impl TextFormat {
    /// `source_location` appends the emitting `file:line`, as
    /// `<PREFIX>_LOG_SOURCE_LOCATION` does.
    pub fn new(source_location: bool) -> Self {
        Self { source_location }
    }
}

impl<S, N> FormatEvent<S, N> for TextFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let ansi = writer.has_ansi_escapes();
        let source = EventSource::of(event, self.source_location);

        dimmed(&mut writer, ansi, write_timestamp)?;
        writer.write_char(' ')?;

        let (colour, label) = level_style(event.metadata().level());
        if ansi {
            writer.write_str(colour)?;
            writer.write_str(label)?;
            writer.write_str(ANSI_RESET)?;
        } else {
            writer.write_str(label)?;
        }
        writer.write_char(' ')?;

        dimmed(&mut writer, ansi, |writer| {
            writer.write_str(&source.target)?;
            writer.write_char(':')
        })?;
        writer.write_char(' ')?;

        if self.source_location && (source.file.is_some() || source.line.is_some()) {
            dimmed(&mut writer, ansi, |writer| source.write_location(writer))?;
            writer.write_char(' ')?;
        }

        write_event_fields(&mut writer, event)?;

        with_current_correlation(|correlation| {
            write!(
                writer,
                " {}={} {}={}",
                field::TRACE_ID,
                correlation.trace_id(),
                field::SPAN_ID,
                correlation.span_id()
            )?;
            if let Some(actor_id) = correlation.actor_id() {
                writer.write_str(" ")?;
                writer.write_str(field::ACTOR_ID)?;
                writer.write_char('=')?;
                write_text_value(&mut writer, actor_id)?;
            }
            Ok(())
        })?;
        writer.write_char('\n')
    }
}

/// `value` bare when it is only ASCII alphanumerics and `-_.@:/+`, debug-quoted
/// otherwise: an `actor_id` can come off the wire (CWE-117).
fn write_text_value(writer: &mut Writer<'_>, value: &str) -> fmt::Result {
    let bare = !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '@' | ':' | '/' | '+')
        });
    if bare {
        writer.write_str(value)
    } else {
        write!(writer, "{value:?}")
    }
}

/// The console format for **JSON** output — [`TextFormat`]'s rule, serialized.
///
/// `trace_id`, `span_id`, `trace_flags` and `actor_id` are top-level keys; the
/// others are `tracing-subscriber`'s own — `timestamp`, `level`, `fields`,
/// `target`, `filename`, `line_number`.
#[derive(Clone, Copy, Debug)]
pub struct JsonFormat {
    source_location: bool,
}

impl JsonFormat {
    /// `source_location` adds the emitting `filename` and `line_number`, as
    /// `<PREFIX>_LOG_SOURCE_LOCATION` does.
    pub fn new(source_location: bool) -> Self {
        Self { source_location }
    }
}

/// Bound to [`JsonFields`] so a layer pairing this format with the plain-text
/// field formatter is a compile error.
impl<S> FormatEvent<S, JsonFields> for JsonFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, JsonFields>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let source = EventSource::of(event, self.source_location);

        writer.write_str("{\"timestamp\":\"")?;
        write_timestamp(&mut writer)?;
        writer.write_str("\",\"level\":\"")?;
        writer.write_str(event.metadata().level().as_str())?;
        writer.write_str("\",\"fields\":")?;
        write_json_fields(&mut writer, event)?;

        writer.write_str(",\"target\":\"")?;
        write_json_escaped(&mut writer, &source.target)?;
        writer.write_char('"')?;

        if self.source_location {
            if let Some(file) = &source.file {
                writer.write_str(",\"filename\":\"")?;
                write_json_escaped(&mut writer, file)?;
                writer.write_char('"')?;
            }
            if let Some(line) = source.line {
                write!(writer, ",\"line_number\":{line}")?;
            }
        }

        with_current_correlation(|correlation| {
            write!(
                writer,
                ",\"{}\":\"{}\",\"{}\":\"{}\",\"{}\":\"{}\"",
                field::TRACE_ID,
                correlation.trace_id(),
                field::SPAN_ID,
                correlation.span_id(),
                field::TRACE_FLAGS,
                correlation.flags()
            )?;
            if let Some(actor_id) = correlation.actor_id() {
                writer.write_str(",\"")?;
                writer.write_str(field::ACTOR_ID)?;
                writer.write_str("\":\"")?;
                write_json_escaped(&mut writer, actor_id)?;
                writer.write_char('"')?;
            }
            Ok(())
        })?;
        writer.write_str("}\n")
    }
}

/// Render an event's own fields as one JSON object in [`JsonFields`]' shape,
/// written here because its visitor has no `record_error` and drops the chain.
fn write_json_fields(writer: &mut Writer<'_>, event: &Event<'_>) -> fmt::Result {
    let mut visitor = JsonFieldWriter {
        writer: writer.by_ref(),
        fields: event.metadata().fields(),
        first: true,
        result: Ok(()),
    };
    visitor.writer.write_char('{')?;
    event.record(&mut visitor);
    visitor.result?;
    writer.write_char('}')
}

struct JsonFieldWriter<'w> {
    writer: Writer<'w>,
    fields: &'static tracing::field::FieldSet,
    first: bool,
    result: fmt::Result,
}

impl JsonFieldWriter<'_> {
    /// Not after an error, and not when a later field carries the same name, so
    /// the last value wins without buffering.
    fn writes(&self, field: &Field) -> bool {
        self.result.is_ok()
            && !self
                .fields
                .iter()
                .any(|other| other.name() == field.name() && other.index() > field.index())
    }

    fn key(&mut self, field: &Field) -> fmt::Result {
        if !std::mem::take(&mut self.first) {
            self.writer.write_char(',')?;
        }
        self.writer.write_char('"')?;
        write_json_escaped(&mut self.writer, field.name())?;
        self.writer.write_str("\":")
    }

    fn raw(&mut self, field: &Field, value: impl fmt::Display) {
        if !self.writes(field) {
            return;
        }
        self.result = self
            .key(field)
            .and_then(|()| write!(self.writer, "{value}"));
    }

    fn string(&mut self, field: &Field, value: impl fmt::Display) {
        if !self.writes(field) {
            return;
        }
        self.result = self.key(field).and_then(|()| {
            self.writer.write_char('"')?;
            write!(JsonEscaping(&mut self.writer), "{value}")?;
            self.writer.write_char('"')
        });
    }
}

impl Visit for JsonFieldWriter<'_> {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.raw(field, value);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.raw(field, value);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.raw(field, value);
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if value.is_finite() {
            // `Debug` keeps a whole float's `.0`, so a backend types it a float.
            self.raw(field, format_args!("{value:?}"));
        } else {
            self.raw(field, "null");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.string(field, value);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.string(field, crate::error_message(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.string(field, format_args!("{value:?}"));
    }
}

struct JsonEscaping<'a, 'w>(&'a mut Writer<'w>);

impl fmt::Write for JsonEscaping<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        write_json_escaped(self.0, text)
    }
}

/// The JSON string-body escapes, plus every character [`LineSafe`] escapes, as
/// `\uXXXX`. Not `serde_json`: it allocates per value or wants an `io::Write`.
fn write_json_escaped(writer: &mut Writer<'_>, value: &str) -> fmt::Result {
    fn escape(ch: char) -> Option<&'static str> {
        match ch {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            _ => None,
        }
    }

    let mut run = 0;
    for (at, ch) in value.char_indices() {
        if let Some(escaped) = escape(ch) {
            writer.write_str(&value[run..at])?;
            writer.write_str(escaped)?;
            run = at + ch.len_utf8();
        } else if forges(ch) {
            writer.write_str(&value[run..at])?;
            write!(writer, "\\u{:04x}", ch as u32)?;
            run = at + ch.len_utf8();
        }
    }
    writer.write_str(&value[run..])
}

/// Where an event says it came from — its own metadata, or, for an event that
/// arrived through the `log` bridge, the record's `log.*` fields.
///
/// This replaces `tracing_log::NormalizeEvent`, whose crate fails the
/// freshness rule of `manifests-ci.md`.
struct EventSource {
    target: Cow<'static, str>,
    file: Option<Cow<'static, str>>,
    line: Option<u32>,
    /// Off skips allocating a bridged event's `log.file` nobody prints.
    want_location: bool,
}

impl EventSource {
    fn of(event: &Event<'_>, source_location: bool) -> Self {
        let meta = event.metadata();
        let mut source = Self {
            target: Cow::Borrowed(meta.target()),
            file: meta.file().map(Cow::Borrowed),
            line: meta.line(),
            want_location: source_location,
        };
        if meta.target() == "log" && meta.name() == "log event" {
            event.record(&mut source);
        }
        source
    }

    /// `file:line:`, each half written only if the source knew it.
    fn write_location(&self, writer: &mut Writer<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            writer.write_str(file)?;
            writer.write_char(':')?;
        }
        if let Some(line) = self.line {
            write!(writer, "{line}:")?;
        }
        Ok(())
    }
}

impl Visit for EventSource {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "log.target" => self.target = Cow::Owned(value.to_owned()),
            "log.file" if self.want_location => {
                self.file = Some(Cow::Owned(value.to_owned()));
            }
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "log.line" {
            self.line = u32::try_from(value).ok();
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {}
}

/// Build the filter from `<PREFIX>_LOG` / `RUST_LOG` / `"info"`; a set-but-
/// unparseable directive is an error, never the default.
#[expect(
    clippy::disallowed_methods,
    reason = "the logging bootstrap runs before any config exists"
)]
fn filter_from_env() -> Result<EnvFilter> {
    let log_var = EnvPrefix::var(var::FILTER);
    let (var, spec) = match std::env::var(&log_var) {
        Ok(v) => (log_var.as_str(), v),
        Err(_) => match std::env::var("RUST_LOG") {
            Ok(v) => ("RUST_LOG", v),
            Err(_) => ("", "info".to_owned()),
        },
    };
    EnvFilter::try_new(&spec)
        .map_err(|e| anyhow::anyhow!("invalid log filter {spec:?} (from {var}): {e}"))
}

/// Install the fallback console subscriber unless one is already set.
pub(crate) fn init_fallback() -> Result<()> {
    if tracing::dispatcher::has_been_set() {
        return Ok(());
    }
    let filter = filter_from_env()?;
    let source_location = bool_from_env(&EnvPrefix::var(var::SOURCE_LOCATION));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    #[expect(
        clippy::disallowed_methods,
        reason = "the logging bootstrap runs before any config exists"
    )]
    let format = std::env::var(EnvPrefix::var(var::FORMAT)).ok();
    #[expect(
        clippy::let_underscore_must_use,
        reason = "an Err is a subscriber already installed, which the fallback steps aside for"
    )]
    let _ = match LogFormat::resolve(format.as_deref()) {
        LogFormat::Text => builder
            .event_format(TextFormat::new(source_location))
            .try_init(),
        // `.json()` sets the field formatter only; the envelope is `JsonFormat`'s.
        LogFormat::Json => builder
            .json()
            .event_format(JsonFormat::new(source_location))
            .try_init(),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::Layer;
    use tracing_subscriber::Registry;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::__private::set_actor_id;
    use crate::Correlation;
    use crate::request_scope::with_request_scope;

    const ACTOR: &str = "01a0112ce24e75509be691162cbbab1f";

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        fn take(&self) -> String {
            let bytes = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
            String::from_utf8(bytes).expect("the formatter writes UTF-8")
        }
    }

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Emitted under the deepest span nesting the framework produces.
    fn emit_service_event() {
        let request = tracing::info_span!(
            "http.request",
            http.request.method = "POST",
            url.path = "/posts",
            client.address = "127.0.0.1"
        );
        let _request = request.enter();
        let operation = tracing::info_span!("mcp.operation");
        let _operation = operation.enter();
        tracing::debug!(target: "fixture::lane", title = "hello", "creating post");
    }

    async fn render(
        layer: impl tracing_subscriber::Layer<Registry> + Send + Sync + 'static,
        captured: Captured,
        actor: &str,
    ) -> (String, String, String) {
        let correlation = Correlation::minted(None);
        let trace_id = correlation.trace_id().to_string();
        let span_id = correlation.span_id().to_string();
        with_request_scope(None, correlation, async {
            set_actor_id(actor);
            tracing::subscriber::with_default(Registry::default().with(layer), emit_service_event);
        })
        .await;
        (captured.take(), trace_id, span_id)
    }

    async fn render_text(source_location: bool, ansi: bool) -> (String, String, String) {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .event_format(TextFormat::new(source_location))
            .with_ansi(ansi)
            .with_writer(captured.clone());
        render(layer, captured, ACTOR).await
    }

    async fn render_json(source_location: bool, actor: &str) -> (String, String, String) {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .event_format(JsonFormat::new(source_location))
            .with_writer(captured.clone());
        render(layer, captured, actor).await
    }

    /// Outside the edge vocabulary, so it copies no real unit's literal.
    const FIXTURE_UNIT: &str = "fixture.line";

    /// A fixture stands in for a target; it may not be a live one.
    const FIXTURE_TARGET: &str = "fixture::lane";

    /// A plain `f64` beside the duration catches padding applied too widely.
    fn emit_operation_line(duration: f64) {
        tracing::info!(
            name: FIXTURE_UNIT,
            target: crate::operation_log::TARGET,
            message = FIXTURE_UNIT,
            method = "close_idle_views",
            outcome = crate::operation_log::OK,
            duration_ms = duration,
            backlog_ratio = 0.5,
        );
    }

    fn render_operation_line(format: LogFormat, duration: f64) -> String {
        let captured = Captured::default();
        let subscriber = match format {
            LogFormat::Text => Registry::default().with(
                tracing_subscriber::fmt::layer()
                    .event_format(TextFormat::new(false))
                    .with_ansi(false)
                    .with_writer(captured.clone())
                    .boxed(),
            ),
            LogFormat::Json => Registry::default().with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .event_format(JsonFormat::new(false))
                    .with_writer(captured.clone())
                    .boxed(),
            ),
        };
        tracing::subscriber::with_default(subscriber, || emit_operation_line(duration));
        captured.take()
    }

    #[test]
    fn the_console_pads_a_duration_to_the_resolution_the_formula_measures() {
        for (duration, rendered) in [
            (0.0, "0.000"),
            (0.04, "0.040"),
            (0.1, "0.100"),
            (12.0, "12.000"),
            (43.783, "43.783"),
        ] {
            let line = render_operation_line(LogFormat::Text, duration);
            assert!(
                line.contains(&format!("duration_ms={rendered} ")),
                "{duration} rendered wider or narrower than its peers: {line}",
            );
        }
    }

    #[test]
    fn no_other_number_on_the_line_is_padded() {
        let line = render_operation_line(LogFormat::Text, 0.04);

        assert!(line.contains("backlog_ratio=0.5\n"), "{line}");
        assert!(line.contains(r#"outcome="ok""#), "{line}");
        assert!(
            line.contains(&format!(
                "{}: {}",
                crate::operation_log::TARGET,
                FIXTURE_UNIT
            )),
            "{line}",
        );
    }

    #[test]
    fn the_json_record_keeps_the_duration_as_a_number() {
        let line = render_operation_line(LogFormat::Json, 0.04);

        let parsed: serde_json::Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|err| panic!("{err}\n{line}"));
        assert_eq!(
            parsed["fields"]["duration_ms"].as_f64(),
            Some(0.04),
            "{line}"
        );
    }

    #[tokio::test]
    async fn a_service_line_carries_the_trace_context_of_its_unit_of_work() {
        let (line, trace_id, span_id) = render_text(false, false).await;

        assert!(
            line.contains(r#"fixture::lane: creating post title="hello""#),
            "{line}"
        );
        assert!(line.contains(&format!("trace_id={trace_id}")), "{line}");
        assert!(line.contains(&format!("span_id={span_id}")), "{line}");
        assert!(line.contains(&format!("actor_id={ACTOR}")), "{line}");
    }

    #[tokio::test]
    async fn a_service_line_carries_nothing_of_the_spans_above_it() {
        let (line, ..) = render_text(false, false).await;

        assert!(!line.contains("http.request"), "{line}");
        assert!(!line.contains("mcp.operation"), "{line}");
        assert!(!line.contains("url.path"), "{line}");
        assert!(!line.contains("client.address"), "{line}");
    }

    #[tokio::test]
    async fn nesting_cannot_duplicate_what_the_line_carries() {
        let (line, ..) = render_text(false, false).await;

        assert_eq!(line.matches("trace_id=").count(), 1, "{line}");
        assert_eq!(line.matches("span_id=").count(), 1, "{line}");
        assert_eq!(line.matches("actor_id=").count(), 1, "{line}");
    }

    #[test]
    fn a_field_value_cannot_forge_a_second_line_or_hide_one() {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .event_format(TextFormat::new(false))
            .with_ansi(false)
            .with_writer(captured.clone());
        let forged = "chat\n2026-01-01T00:00:00Z  WARN nest_rs::authn: forged \u{202E}\u{1b}[2J";
        tracing::subscriber::with_default(Registry::default().with(layer), || {
            tracing::warn!(target: "fixture::lane", event = %forged, "unknown event");
        });
        let line = captured.take();

        assert_eq!(line.trim_end().lines().count(), 1, "{line}");
        assert!(line.contains("\\n"), "{line}");
        assert!(!line.contains('\u{202E}'), "{line}");
        assert!(!line.contains('\u{1b}'), "{line}");
    }

    #[tokio::test]
    async fn an_event_outside_a_unit_of_work_carries_no_ids() {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .event_format(TextFormat::new(false))
            .with_ansi(false)
            .with_writer(captured.clone());
        tracing::subscriber::with_default(Registry::default().with(layer), emit_service_event);
        let line = captured.take();

        assert!(line.contains("fixture::lane: creating post"), "{line}");
        assert!(!line.contains("trace_id"), "{line}");
        assert!(!line.contains("actor_id"), "{line}");
    }

    #[tokio::test]
    async fn the_json_record_carries_the_ids_at_the_top_level() {
        let (line, trace_id, span_id) = render_json(false, ACTOR).await;

        assert!(
            line.contains(&format!(r#""trace_id":"{trace_id}""#)),
            "{line}"
        );
        assert!(
            line.contains(&format!(r#""span_id":"{span_id}""#)),
            "{line}"
        );
        assert!(line.contains(&format!(r#""actor_id":"{ACTOR}""#)), "{line}");
        assert!(line.contains(r#""target":"fixture::lane""#), "{line}");
        assert!(!line.contains(r#""span""#), "{line}");
        assert!(!line.contains(r#""spans""#), "{line}");
        assert!(!line.contains("url.path"), "{line}");
    }

    #[tokio::test]
    async fn the_json_envelope_escapes_what_came_off_the_wire() {
        let (line, ..) = render_json(false, "a\"quote\"\nand a newline").await;

        assert!(
            line.contains(r#""actor_id":"a\"quote\"\nand a newline""#),
            "{line}"
        );
    }

    #[tokio::test]
    async fn the_text_format_quotes_what_came_off_the_wire() {
        async fn text_line(actor: &str) -> String {
            let captured = Captured::default();
            let layer = tracing_subscriber::fmt::layer()
                .event_format(TextFormat::new(false))
                .with_ansi(false)
                .with_writer(captured.clone());
            render(layer, captured, actor).await.0
        }

        let forged_line =
            text_line("user-7\n2026-09-15T00:00:00Z  WARN nest_rs::authz: forged\u{1b}[2K\\").await;
        assert_eq!(
            forged_line.matches('\n').count(),
            1,
            "one line: {forged_line}"
        );
        assert!(
            forged_line.contains(
                r#"actor_id="user-7\n2026-09-15T00:00:00Z  WARN nest_rs::authz: forged\u{1b}[2K\\""#
            ),
            "{forged_line}"
        );

        let forged_ids = text_line("alice trace_id=00000000000000000000000000000000").await;
        assert!(
            forged_ids.ends_with("actor_id=\"alice trace_id=00000000000000000000000000000000\"\n"),
            "{forged_ids}"
        );

        let unseen = text_line("a\u{2028}b\u{85}c\u{9b}d\u{202e}e\u{200b}f").await;
        assert!(
            unseen.contains(r#"actor_id="a\u{2028}b\u{85}c\u{9b}d\u{202e}e\u{200b}f""#),
            "{unseen}"
        );

        let usual = text_line("alice-42@example.com").await;
        assert!(
            usual.ends_with("actor_id=alice-42@example.com\n"),
            "{usual}"
        );
    }

    #[tokio::test]
    async fn source_location_is_the_formatter_s_knob_now_that_the_builder_s_is_inert() {
        assert!(!render_text(false, false).await.0.contains("logging.rs"));
        assert!(render_text(true, false).await.0.contains("logging.rs:"));
        assert!(
            render_json(true, ACTOR)
                .await
                .0
                .contains(r#""filename":"crates/nest-rs-core/src/logging.rs""#)
        );
    }

    #[tokio::test]
    async fn ansi_follows_the_writer() {
        assert!(!render_text(false, false).await.0.contains('\u{1b}'));
        assert!(render_text(false, true).await.0.contains(ANSI_DIM));
    }

    /// The envelope is hand-written; these are the values that break a naive escaper.
    #[tokio::test]
    async fn every_json_line_parses_however_hostile_the_values() {
        const HOSTILE: &[&str] = &[
            "\"",
            "\\",
            "\\\"",
            "\"\"",
            "\u{0}",
            "\u{1f}",
            "\u{7f}",
            "\ttab\tends\t",
            "é\"é",
            "\"leading",
            "trailing\"",
            "line\nbreak\r\nand\ttab",
            "🙂\\🙂",
        ];

        for actor in HOSTILE {
            let (line, ..) = render_json(true, actor).await;
            let parsed: serde_json::Value = serde_json::from_str(line.trim())
                .unwrap_or_else(|err| panic!("{actor:?} produced unparseable JSON: {err}\n{line}"));
            assert_eq!(
                parsed["actor_id"].as_str(),
                Some(*actor),
                "the value round-trips rather than merely surviving: {line}",
            );
            assert_eq!(parsed["target"].as_str(), Some(FIXTURE_TARGET));
            assert!(parsed["fields"].is_object(), "{line}");
        }
    }

    #[test]
    fn a_message_recorded_as_a_str_cannot_forge_a_second_line() {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .event_format(TextFormat::new(false))
            .with_ansi(false)
            .with_writer(captured.clone());
        let forged = "a\nFORGED WARN x \u{202E}";
        tracing::subscriber::with_default(Registry::default().with(layer), || {
            tracing::info!(target: "fixture::lane", message = forged);
        });
        let line = captured.take();

        assert_eq!(line.trim_end().lines().count(), 1, "{line}");
        assert!(!line.contains('\u{202E}'), "{line}");
    }

    #[derive(Debug)]
    struct Outer(std::io::Error);

    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer failed")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn a_json_error_field_carries_its_causes_and_hides_nothing() {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .event_format(JsonFormat::new(false))
            .with_writer(captured.clone());
        tracing::subscriber::with_default(Registry::default().with(layer), || {
            let error = Outer(std::io::Error::other("disk \u{202E}full"));
            tracing::error!(
                target: "fixture::lane",
                error = &error as &(dyn std::error::Error + 'static),
                count = 3u64,
                "write failed"
            );
        });
        let line = captured.take();

        assert!(!line.contains('\u{202E}'), "{line}");
        let parsed: serde_json::Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|err| panic!("{err}\n{line}"));
        assert_eq!(
            parsed["fields"]["error"].as_str(),
            Some("outer failed: disk \u{202E}full"),
            "{line}"
        );
        assert_eq!(parsed["fields"]["count"].as_u64(), Some(3), "{line}");
        assert_eq!(
            parsed["fields"]["message"].as_str(),
            Some("write failed"),
            "{line}"
        );
    }

    #[test]
    fn json_fields_keep_the_shape_json_fields_wrote() {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .event_format(JsonFormat::new(false))
            .with_writer(captured.clone());
        tracing::subscriber::with_default(Registry::default().with(layer), || {
            tracing::info!(
                target: "fixture::lane",
                whole = 12.0_f64,
                nan = f64::NAN,
                a = 1u64,
                a = 2u64,
                "shape"
            );
        });
        let line = captured.take();

        assert!(line.contains("\"whole\":12.0"), "{line}");
        let parsed: serde_json::Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|err| panic!("{err}\n{line}"));
        assert!(parsed["fields"]["nan"].is_null(), "{line}");
        assert_eq!(parsed["fields"]["a"].as_u64(), Some(2), "{line}");
        assert_eq!(line.matches("\"a\":").count(), 1, "{line}");
    }

    #[test]
    fn a_json_line_outside_a_unit_of_work_still_parses() {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .event_format(JsonFormat::new(false))
            .with_writer(captured.clone());
        tracing::subscriber::with_default(Registry::default().with(layer), emit_service_event);
        let line = captured.take();

        let parsed: serde_json::Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|err| panic!("{err}\n{line}"));
        assert!(parsed.get("trace_id").is_none(), "{line}");
        assert!(parsed.get("actor_id").is_none(), "{line}");
        assert_eq!(parsed["level"].as_str(), Some("DEBUG"), "{line}");
    }

    /// `line` is captured whatever the flag says, `file` is not.
    #[test]
    fn a_half_known_source_location_renders_without_a_dangling_separator() {
        let mut source = EventSource {
            target: Cow::Borrowed(FIXTURE_TARGET),
            file: None,
            line: Some(42),
            want_location: true,
        };
        let mut buf = String::new();
        source
            .write_location(&mut Writer::new(&mut buf))
            .expect("writing to a string cannot fail");
        assert_eq!(buf, "42:", "a line with no file is still legible: {buf:?}");

        buf.clear();
        source.line = None;
        source.file = Some(Cow::Borrowed("src/lib.rs"));
        source
            .write_location(&mut Writer::new(&mut buf))
            .expect("writing to a string cannot fail");
        assert_eq!(buf, "src/lib.rs:", "{buf:?}");
    }

    #[test]
    fn format_resolves_canonical_names_case_insensitively() {
        assert_eq!(LogFormat::resolve(Some("json")), LogFormat::Json);
        assert_eq!(LogFormat::resolve(Some("JSON")), LogFormat::Json);
        assert_eq!(LogFormat::resolve(Some("  text  ")), LogFormat::Text);
    }

    #[test]
    fn format_defaults_by_build_profile_when_absent_or_unrecognized() {
        let expected = if cfg!(debug_assertions) {
            LogFormat::Text
        } else {
            LogFormat::Json
        };
        assert_eq!(LogFormat::resolve(None), expected);
        assert_eq!(LogFormat::resolve(Some("yaml")), expected);
    }

    #[test]
    fn a_valid_filter_directive_parses() {
        assert!(EnvFilter::try_new("debug,hyper=warn").is_ok());
    }

    #[test]
    fn an_invalid_filter_directive_is_rejected() {
        assert!(EnvFilter::try_new("foo=notalevel").is_err());
    }

    /// `EnvFilter` matches a target by raw `starts_with`, not by `::` segment —
    /// the property the no-prefix target rule rests on. Proving it needs a
    /// strict prefix of a real target, one nothing declares.
    #[test]
    fn a_directive_matches_a_target_by_raw_prefix_not_by_segment() {
        let family = crate::operation_log::TARGET;
        let prefix = &family[..family.len() - 5];
        assert!(
            family.starts_with(prefix) && prefix != family,
            "the fixture must be a strict prefix of a real target: {prefix} / {family}",
        );

        let captured = Captured::default();
        let subscriber = Registry::default()
            .with(EnvFilter::new(format!("info,{prefix}=off")))
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(TextFormat::new(false))
                    .with_ansi(false)
                    .with_writer(captured.clone()),
            );
        tracing::subscriber::with_default(subscriber, || emit_operation_line(0.5));

        assert!(
            captured.take().is_empty(),
            "`{prefix}=off` must silence `{family}` — a segment matcher would let it \
             through, and the no-prefix rule would guard a property nothing has",
        );
    }

    #[test]
    fn the_family_toggle_leaves_a_target_it_does_not_prefix_alone() {
        let captured = Captured::default();
        let subscriber = Registry::default()
            .with(EnvFilter::new(format!(
                "info,{}=off",
                crate::operation_log::TARGET
            )))
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(TextFormat::new(false))
                    .with_ansi(false)
                    .with_writer(captured.clone()),
            );

        tracing::subscriber::with_default(subscriber, || {
            emit_operation_line(0.5);
            tracing::warn!(
                target: crate::target::APP,
                phase = "boot",
                "a neighbour the family toggle must leave alone",
            );
        });

        let out = captured.take();
        assert!(!out.contains(FIXTURE_UNIT), "the family is off: {out}");
        assert!(
            out.contains("a neighbour the family toggle must leave alone"),
            "a target the family does not prefix is not the family's to silence: {out}",
        );
    }
}

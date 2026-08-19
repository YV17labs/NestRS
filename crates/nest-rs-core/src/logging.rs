//! Baseline console logging, installed at boot when no global `tracing`
//! subscriber is set — so a bare app logs out of the box, with zero
//! per-request cost beyond `tracing`'s own filtering.
//!
//! An observability stack (e.g. `nest-rs-opentelemetry`) installs its richer
//! subscriber in `main` *before* the app builds; this fallback detects it and
//! steps aside. Configuration is env-only:
//!
//! - `NESTRS_LOG` (falling back to `RUST_LOG`) — `EnvFilter` directives,
//!   default `info`. Set-but-unparseable aborts boot, the same posture as
//!   every other framework var.
//! - `NESTRS_LOG_FORMAT` — `text` or `json`; defaults by build profile
//!   (text in debug, JSON in release), unrecognized values keep the default.
//! - `NESTRS_LOG_SOURCE_LOCATION` — append the emitting `file:line` to each
//!   event; off by default (widens every line, leaks source paths in prod).
//!
//! `NESTRS` is the default prefix; under
//! [`EnvPrefix::VAR`](crate::EnvPrefix::VAR) the three become `<PREFIX>_LOG*`.
//! `RUST_LOG` is not prefixed — it is the ecosystem's variable, not ours.
//!
//! The same three variables drive the console layer of any richer subscriber
//! the framework ships (`nest-rs-opentelemetry`), so an app's log config
//! survives adopting or dropping the observability stack unchanged.
//!
//! Behind the default `logging` cargo feature — an embedder that installs its
//! own subscriber can disable it and drop `tracing-subscriber` from the
//! kernel's dependency tree entirely. Not an app running the observability
//! stack: `nest-rs-opentelemetry` enables the feature for
//! [`TextFormat`]/[`JsonFormat`], and links `tracing-subscriber` directly
//! regardless, so there is nothing that hatch could save it.

use std::borrow::Cow;
use std::fmt;

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
use crate::request_scope::current_request_ctx;
use crate::trace_context::Correlation;

/// The tails of the framework-wide logging variables, joined with the
/// deployment's prefix by [`EnvPrefix::var`](crate::EnvPrefix::var).
///
/// Declared because `EnvPrefix::var` builds the *prefix* and leaves the name to
/// its caller, so nothing held these: `LOG`, `LOG_FORMAT` and
/// `LOG_SOURCE_LOCATION` were re-typed at seven sites in three crates, and this
/// family has **two** subscribers that must agree — the kernel's fallback logger
/// and `nest-rs-opentelemetry`'s. A rename in one of them changes behaviour in
/// that one alone, and `EnvPrefix::var("LOG_FORMATT")` compiles and reads
/// nothing, forever.
///
/// The precedent is one crate over and states the rule: `Environment::var_name`
/// is public "because a harness that must decide the environment before the
/// framework does has to name the same variable, **and a second literal there is
/// exactly how a rename half-lands**".
///
/// `nest-rs-cli` keeps its own spelling in the scaffold templates, and that is
/// forced rather than excused — it links no `nest-rs-*` crate, the same reason
/// it mirrors `var_name`.
pub mod var {
    /// `<PREFIX>_LOG` — the `EnvFilter` directive the process logs under.
    pub const FILTER: &str = "LOG";
    /// `<PREFIX>_LOG_FORMAT` — `text` or `json`.
    pub const FORMAT: &str = "LOG_FORMAT";
    /// `<PREFIX>_LOG_SOURCE_LOCATION` — whether a line carries file and line.
    pub const SOURCE_LOCATION: &str = "LOG_SOURCE_LOCATION";
}

/// Shape of the console log layer's output.
///
/// **The grammar of `<PREFIX>_LOG_FORMAT` is worded here and nowhere else.** It
/// used to be spelled twice — once for the kernel's fallback subscriber, once in
/// `nest-rs-opentelemetry` — with two parsers and two build-profile defaults, so
/// a new value or a new tolerance could reach one console and not the other, and
/// an app's log shape could change on adopting the exporter. That is the failure
/// this crate's own comments say must not happen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable pretty-print for local development. The default in debug
    /// builds; pinning it is what keeps default apps readable at the terminal.
    #[default]
    Text,
    /// One JSON object per event for log aggregators. The default in release
    /// builds so a deployed app is machine-parseable without extra config.
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

    /// Text in debug, JSON in release — so a deployed app is machine-parseable
    /// and a development one is readable, neither needing the variable set.
    pub fn by_profile() -> Self {
        if cfg!(debug_assertions) {
            Self::Text
        } else {
            Self::Json
        }
    }

    /// [`parse`](Self::parse) falling back to [`by_profile`](Self::by_profile) —
    /// what a caller with no error path to offer wants.
    pub fn resolve(raw: Option<&str>) -> Self {
        raw.and_then(Self::parse).unwrap_or_else(Self::by_profile)
    }
}

/// [`parse_bool`](crate::parse_bool) against a named variable, `false` when it is unset or
/// unrecognized.
fn bool_from_env(name: &str) -> bool {
    std::env::var(name).ok().and_then(|v| crate::parse_bool(&v)) == Some(true)
}

/// SGR sequences, spelled out rather than pulled from a colour crate:
/// tracing-subscriber's own `Style` is private to its `fmt::format` module, so
/// the alternative to these constants is a direct dependency for them.
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";

/// The colour and the five-column label `Format<Full>` gives each level, kept
/// byte-for-byte: what changed is what a line *says*, never how a terminal reads
/// it.
fn level_style(level: &Level) -> (&'static str, &'static str) {
    match *level {
        Level::TRACE => ("\x1b[35m", "TRACE"),
        Level::DEBUG => ("\x1b[34m", "DEBUG"),
        Level::INFO => ("\x1b[32m", " INFO"),
        Level::WARN => ("\x1b[33m", " WARN"),
        Level::ERROR => ("\x1b[31m", "ERROR"),
    }
}

/// A clock that cannot be read is not worth failing a log line over —
/// `Format<Full>` bails only on formatting errors, and so does this.
fn write_timestamp(writer: &mut Writer<'_>) -> fmt::Result {
    if SystemTime.format_time(writer).is_err() {
        writer.write_str("<unknown time>")?;
    }
    Ok(())
}

/// Write `body` dimmed when the writer takes colour, plain when it does not.
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

/// Run `write` against the correlation of the unit of work this event belongs
/// to, or write nothing where there is none.
///
/// **One task-local read, and the actor is borrowed rather than cloned** — this
/// runs on every event the process emits, and
/// [`current_actor_id`](crate::current_actor_id) hands back an owned `String`
/// that a log line would allocate and drop within the same statement.
fn with_current_correlation(write: impl FnOnce(&Correlation) -> fmt::Result) -> fmt::Result {
    current_request_ctx(|ctx| write(&ctx.correlation)).unwrap_or(Ok(()))
}

/// Render an event's own fields, with the operation line's `duration_ms` padded
/// to every digit of its resolution.
///
/// A duration is a **column**: an operator reads a screen of
/// [`operation_log::TARGET`](crate::operation_log::TARGET) lines down the page,
/// and `0.04` beside `0.043` is the same number to a machine but two widths to a
/// human, so the whole column has to be re-parsed by eye. Padding to
/// [`DURATION_DECIMALS`](crate::operation_log::DURATION_DECIMALS) is what makes
/// it scannable, and it is honest because that is the resolution the formula
/// already rounds to.
///
/// **JSON does not pad**, the third recorded asymmetry beside `trace_flags` and
/// `parent_span_id`: trailing zeros are a reader's affordance, and the record is
/// read by a machine that would have to strip them to get the number back.
///
/// The visitor delegates to tracing-subscriber's own [`DefaultVisitor`] rather
/// than reimplementing it — ANSI escaping, the `log.*` skip and the quoting rules
/// stay theirs, and one field is intercepted on the way through. That is also why
/// [`TextFormat`] renders fields here instead of through the layer's
/// `FormatFields`: routing through it would make the padding depend on every
/// mount site passing a matching `fmt_fields`, and this format already owns every
/// other choice on the line.
fn write_event_fields(writer: &mut Writer<'_>, event: &Event<'_>) -> fmt::Result {
    let mut visitor = FixedWidthDurations(DefaultVisitor::new(writer.by_ref(), true));
    event.record(&mut visitor);
    visitor.0.finish()
}

/// See [`write_event_fields`]. Only `record_f64` decides anything; the other
/// methods forward, and the ones left to `Visit`'s defaults land on
/// [`DefaultVisitor::record_debug`] exactly as they would have without the
/// wrapper.
struct FixedWidthDurations<'a>(DefaultVisitor<'a>);

impl Visit for FixedWidthDurations<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == crate::operation_log::DURATION_MS {
            let decimals = crate::operation_log::DURATION_DECIMALS;
            // `Arguments` debug-prints as it displays, which is how
            // `DefaultVisitor` renders a value it has already formatted itself.
            self.0
                .record_debug(field, &format_args!("{value:.decimals$}"));
        } else {
            self.0.record_f64(field, value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.record_str(field, value);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0.record_error(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.record_debug(field, value);
    }
}

/// The console format for **text** output.
///
/// A line is `timestamp level target: message fields`, then the **W3C Trace
/// Context of the unit of work it belongs to** — `trace_id`, `span_id`, and
/// `actor_id` once a guard resolved a principal:
///
/// ```text
/// 2026-08-18T14:49:16.529159Z DEBUG features::posts: creating post title="hello" trace_id=01a014ec214e7163844619af2aeaeca4 span_id=b0c095c368ba3d72
/// ```
///
/// That is the whole correlation mechanism: two lines belong to the same work
/// when the ids match, and nothing here depends on an exporter or a collector.
///
/// # The ids are read from the ambient context, never off the span scope
///
/// **A line carries no span attributes and no span names**, and each reason has
/// already bitten:
///
/// - a span's fields belong to the *span*, so rendering them put an HTTP
///   request's method, path, client address and user agent on every line a
///   service below it emitted — a service's line stopped being about what the
///   service did;
/// - `tracing` renders a whole scope, so a nested unit of work — an MCP
///   operation inside a request, a job inside its enqueue — printed one
///   `trace_id` per level;
/// - the context **outlives the span**. A streaming body is polled after its
///   handler returned and the framework re-installs the request around it, so
///   the ids are readable exactly where a span stack no longer is.
///
/// Span structure itself is untouched — it is what `tracing-opentelemetry`
/// builds the exported tree from.
///
/// # This format owns the choices the builder's `Format` knobs used to make
///
/// `.event_format(…)` replaces `Format` wholesale, so `with_file`,
/// `with_line_number`, `with_timer`, `with_target`, `with_level` and
/// `with_thread_ids` no longer reach the output: the timer is [`SystemTime`],
/// the target and level always print, thread identity never does, and
/// `file:line` is [`source_location`](Self::new) — the one knob both console
/// sites actually set. ANSI still follows the writer, so a layer with colour
/// disabled stays plain.
#[derive(Clone, Copy, Debug)]
pub struct TextFormat {
    source_location: bool,
}

impl TextFormat {
    /// `source_location` appends the emitting `file:line` — the
    /// `<PREFIX>_LOG_SOURCE_LOCATION` variable, passed in rather than read here
    /// so the observability stack can pin it from its own config.
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
                " trace_id={} span_id={}",
                correlation.trace_id(),
                correlation.span_id()
            )?;
            if let Some(actor_id) = correlation.actor_id() {
                writer.write_str(" actor_id=")?;
                writer.write_str(actor_id)?;
            }
            Ok(())
        })?;
        writer.write_char('\n')
    }
}

/// The console format for **JSON** output — [`TextFormat`]'s rule, serialized.
///
/// `trace_id`, `span_id` and `actor_id` are **top-level keys**, which is where
/// the standard puts them and where a backend looks without a per-deployment
/// field mapping. They are not nested under a `span` object, because they do not
/// describe a span: they identify the unit of work the record belongs to.
///
/// The envelope's other keys are `tracing-subscriber`'s own — `timestamp`,
/// `level`, `fields`, `target`, `filename`, `line_number` — so a pipeline
/// reading this output keeps reading it.
///
/// **Every line's ids are top-level, with no exception**, and the paragraph that
/// used to record one here described a shape that was removed on purpose: the
/// HTTP operation line writes no `trace_id`, `span_id` or `actor_id` of its own
/// (`nest_rs_http::access_log` says so at the emit site), because the formatter
/// reads them off the ambient correlation and spelling them as event fields
/// would print them twice in text and put them in two positions in this
/// envelope. The request's context is re-entered around that line for exactly
/// that reason.
#[derive(Clone, Copy, Debug)]
pub struct JsonFormat {
    source_location: bool,
}

impl JsonFormat {
    /// See [`TextFormat::new`] — the same knob, for the same reason.
    pub fn new(source_location: bool) -> Self {
        Self { source_location }
    }
}

/// Bound to [`JsonFields`] rather than generic over the field formatter: the
/// envelope splices the formatted fields in as a JSON **object**, so pairing this
/// with the plain-text field formatter would emit malformed JSON. A bound makes
/// that a compile error instead.
impl<S> FormatEvent<S, JsonFields> for JsonFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, JsonFields>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let source = EventSource::of(event, self.source_location);

        writer.write_str("{\"timestamp\":\"")?;
        write_timestamp(&mut writer)?;
        writer.write_str("\",\"level\":\"")?;
        // `as_str` rather than `Display`, which pads through a `Formatter`.
        writer.write_str(event.metadata().level().as_str())?;
        writer.write_str("\",\"fields\":")?;
        // `JsonFields` writes a complete object, braces included.
        ctx.format_fields(writer.by_ref(), event)?;

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
            // `trace_flags` is the third field the log data model names beside the
            // two ids, and it is emitted here and not in text on purpose: this
            // record is read by a machine that may join it against an export,
            // where the sampling bit decides whether the other half exists. A
            // human at a console never acts on it. Asymmetry recorded in
            // `.claude/rules/`.
            write!(
                writer,
                ",\"trace_id\":\"{}\",\"span_id\":\"{}\",\"trace_flags\":\"{}\"",
                correlation.trace_id(),
                correlation.span_id(),
                correlation.flags()
            )?;
            if let Some(actor_id) = correlation.actor_id() {
                writer.write_str(",\"actor_id\":\"")?;
                write_json_escaped(&mut writer, actor_id)?;
                writer.write_char('"')?;
            }
            Ok(())
        })?;
        writer.write_str("}\n")
    }
}

/// The JSON string-body escapes, and no others: the two mandatory ones plus every
/// control character as `\u00XX`. A log line carries developer-authored targets
/// and an `actor_id` that came off the wire, so this cannot be skipped on the
/// grounds that "it is always a Rust path".
///
/// Written here rather than through `serde_json` — which is in the tree, behind
/// tracing-subscriber's `json` feature — because every reachable spelling of it
/// allocates per value (`to_string`, `Value::String`) or wants an `io::Write`
/// where a [`Writer`] is a `fmt::Write`, and this runs on every event.
///
/// Escape-free is the overwhelming case (two of the three call sites are Rust
/// paths), so it is one `write_str` for the whole value, and otherwise one per
/// run between escapes rather than one per character.
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
        } else if ch.is_control() {
            writer.write_str(&value[run..at])?;
            write!(writer, "\\u{:04x}", ch as u32)?;
            run = at + ch.len_utf8();
        }
    }
    writer.write_str(&value[run..])
}

/// Where an event says it came from — its own metadata, or, for an event that
/// arrived through the `log` bridge, the record's.
///
/// `Format<Full>` normalizes through `tracing_log::NormalizeEvent`, a trait on a
/// crate nothing here declares: it is compiled in already, but its last release
/// predates the floor `CLAUDE.md` sets for adopting one. The bridge's shape is
/// its documented contract — one static callsite per level, named `"log event"`
/// on target `"log"`, carrying the record's real metadata in `log.*` fields — so
/// seeding from the event's own metadata and letting those fields overwrite it is
/// that normalization without the manifest line. Skipping it would file every
/// bridged line under the target `log`.
///
/// Seeding is also what removes the special case: a bridged callsite's metadata
/// already reads `target = "log"` with no file and no line, which is exactly what
/// `NormalizeEvent` falls back to when a record carried none.
struct EventSource {
    target: Cow<'static, str>,
    file: Option<Cow<'static, str>>,
    line: Option<u32>,
    /// Whether the formatter will read [`file`](Self::file) at all. A bridged
    /// event owns its strings, so capturing one the line never prints is an
    /// allocation per event for nothing.
    want_location: bool,
}

impl EventSource {
    /// The metadata comparison is two string compares, paid once per event;
    /// nothing is visited unless the event really came through the bridge.
    ///
    /// `source_location` is threaded in rather than checked at the call site so a
    /// bridged event does not allocate a `log.file` nobody will read: the flag is
    /// off by default, and the visitor is the only place that would own the
    /// string.
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

    /// `file:line:`, each half written only if the source knew it — the shape
    /// `Format<Full>` produces, so a line stays greppable the same way.
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

/// Build the filter from `<PREFIX>_LOG` / `RUST_LOG` / `"info"`. A set-but-
/// unparseable directive is a config error that aborts boot, never a silent
/// downgrade to the default.
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
/// Called by [`App`](crate::App) on both boot paths before the first boot
/// event, so module/route logs render even in a bare app.
pub(crate) fn init_fallback() -> Result<()> {
    if tracing::dispatcher::has_been_set() {
        return Ok(());
    }
    let filter = filter_from_env()?;
    let source_location = bool_from_env(&EnvPrefix::var(var::SOURCE_LOCATION));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    // A lost race against a concurrent install is the "already set" case —
    // the fallback steps aside; it never unseats another subscriber.
    let format = std::env::var(EnvPrefix::var(var::FORMAT)).ok();
    let _ = match LogFormat::resolve(format.as_deref()) {
        // Both branches are the framework's own `FormatEvent`; [`TextFormat`]
        // carries the argument for why, and for why `with_file` /
        // `with_line_number` are not set on either.
        LogFormat::Text => builder
            .event_format(TextFormat::new(source_location))
            .try_init(),
        // `.json()` for the field formatter only — the envelope is
        // [`JsonFormat`]'s.
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
    use crate::Correlation;
    use crate::request_scope::with_request_scope;
    use crate::trace_context::set_actor_id;

    const ACTOR: &str = "01a0112ce24e75509be691162cbbab1f";

    /// A `MakeWriter` handing the layer a buffer, so a test reads back the bytes
    /// a terminal or a log shipper would have received.
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

    /// One service event, emitted under an HTTP request span that itself sits
    /// under an MCP operation span — the deepest nesting the framework produces,
    /// so a formatter that leaks span state leaks it here.
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
        tracing::debug!(target: "features::posts", title = "hello", "creating post");
    }

    /// Renders `emit_service_event` under a real request context, and hands back
    /// the line with the ids that context carried.
    async fn render(
        layer: impl tracing_subscriber::Layer<Registry> + Send + Sync + 'static,
        captured: Captured,
        actor: &str,
    ) -> (String, String, String) {
        let correlation = Correlation::mint();
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

    /// The canonical name of a unit of work belongs to the crate that owns the
    /// edge, so the kernel cannot name one — see `operation_log`. These are
    /// formatter tests, so the name is a **fixture**: a namespace outside the
    /// closed edge vocabulary, deliberately, because copying a real one
    /// (`schedule.tick`) is not standing in for it — it is the same string,
    /// asserted from a file that cannot see the constant, which is the copied
    /// literal `CLAUDE.md` forbids wearing a fixture's name. The `units` join
    /// reads no `#[cfg(test)]` emission, so nothing here joins the vocabulary
    /// either way.
    const FIXTURE_UNIT: &str = "fixture.line";

    /// One operation line, the shape every edge files through
    /// [`operation_log`](crate::operation_log) — a duration and a plain `f64`
    /// beside it, so a formatter that pads too widely is caught by the same
    /// event.
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

    /// The console reads a duration as a **column**, so the digits after the
    /// point cannot come and go with the value: `0.04` and `0.043` are one number
    /// each but two widths, and a screen of ticks is re-parsed by eye.
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

    /// Padding is this one field's, not every float's — a formatter that widened
    /// `f64` as a class would rewrite an app's own numbers on the way past.
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

    /// The recorded asymmetry: trailing zeros are a reader's affordance, so the
    /// machine-read record keeps a JSON **number** that needs no stripping.
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

    /// The requirement in one test: a service line says what the service did and
    /// carries the ids that relate it to everything else on the same work.
    #[tokio::test]
    async fn a_service_line_carries_the_trace_context_of_its_unit_of_work() {
        let (line, trace_id, span_id) = render_text(false, false).await;

        assert!(
            line.contains(r#"features::posts: creating post title="hello""#),
            "{line}"
        );
        assert!(line.contains(&format!("trace_id={trace_id}")), "{line}");
        assert!(line.contains(&format!("span_id={span_id}")), "{line}");
        assert!(line.contains(&format!("actor_id={ACTOR}")), "{line}");
    }

    /// The other half, and the one that regressed: what a service logs is about
    /// the service. The request's own attributes belong to the request.
    #[tokio::test]
    async fn a_service_line_carries_nothing_of_the_spans_above_it() {
        let (line, ..) = render_text(false, false).await;

        assert!(!line.contains("http.request"), "{line}");
        assert!(!line.contains("mcp.operation"), "{line}");
        assert!(!line.contains("url.path"), "{line}");
        assert!(!line.contains("client.address"), "{line}");
    }

    /// Nesting is normal and must cost nothing: the ids come from the context,
    /// which has one value however deep the span stack runs.
    #[tokio::test]
    async fn nesting_cannot_duplicate_what_the_line_carries() {
        let (line, ..) = render_text(false, false).await;

        assert_eq!(line.matches("trace_id=").count(), 1, "{line}");
        assert_eq!(line.matches("span_id=").count(), 1, "{line}");
        assert_eq!(line.matches("actor_id=").count(), 1, "{line}");
    }

    /// Absence is the answer, not a sentinel — an event outside any unit of work
    /// carries no id rather than an empty or invented one.
    #[tokio::test]
    async fn an_event_outside_a_unit_of_work_carries_no_ids() {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .event_format(TextFormat::new(false))
            .with_ansi(false)
            .with_writer(captured.clone());
        tracing::subscriber::with_default(Registry::default().with(layer), emit_service_event);
        let line = captured.take();

        assert!(line.contains("features::posts: creating post"), "{line}");
        assert!(!line.contains("trace_id"), "{line}");
        assert!(!line.contains("actor_id"), "{line}");
    }

    /// JSON says the same thing, at the top level of the record — where a
    /// backend reads it without a per-deployment field mapping.
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
        assert!(line.contains(r#""target":"features::posts""#), "{line}");
        assert!(!line.contains(r#""span""#), "{line}");
        assert!(!line.contains(r#""spans""#), "{line}");
        assert!(!line.contains("url.path"), "{line}");
    }

    /// An `actor_id` came off the wire, so the envelope escapes it. A record that
    /// cannot be parsed is a record that was not kept.
    #[tokio::test]
    async fn the_json_envelope_escapes_what_came_off_the_wire() {
        let (line, ..) = render_json(false, "a\"quote\"\nand a newline").await;

        assert!(
            line.contains(r#""actor_id":"a\"quote\"\nand a newline""#),
            "{line}"
        );
    }

    #[tokio::test]
    async fn source_location_is_the_formatter_s_knob_now_that_the_builder_s_is_inert() {
        // `with_file`/`with_line_number` no longer reach the output, so this is
        // the only spelling left — a silent no-op here would be invisible.
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

    /// Every JSON line the formatter can emit has to parse, and the envelope is
    /// hand-written, so this is the assertion the whole format rests on. The
    /// values are the ones that break a naive escaper: the two mandatory escapes,
    /// raw control characters, multi-byte UTF-8 either side of an escape, an
    /// escape at the first and last byte, and two in a row.
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
            assert_eq!(parsed["target"].as_str(), Some("features::posts"));
            assert!(parsed["fields"].is_object(), "{line}");
        }
    }

    /// The correlation block writes nothing outside a unit of work, so the
    /// envelope has to stay closed and comma-free on that path too — a boot log is
    /// the common case, not an edge case.
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

    /// `line` is captured whatever the flag says while `file` is not, so a source
    /// that knows one and not the other must still render something a reader can
    /// parse rather than a dangling separator.
    #[test]
    fn a_half_known_source_location_renders_without_a_dangling_separator() {
        let mut source = EventSource {
            target: Cow::Borrowed("features::posts"),
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
        // The boot path maps this to an error naming the variable — a set-but-
        // unparseable filter must abort, not degrade to `info`.
        assert!(EnvFilter::try_new("foo=notalevel").is_err());
    }

    /// `EnvFilter` matches a directive against an event's target with
    /// `starts_with` on the **raw string**, not on `::` segments.
    ///
    /// This is a property of `tracing-subscriber`, not of our tree, and the
    /// `filters` join in `nest-rs-conformance` rests entirely on it: if a
    /// version ever matched by segment, that join's whole subject would
    /// disappear with nothing to say so. It shipped as a defect once —
    /// `nest_rs::access`, the family's target through 5.1, prefixed
    /// `nest_rs::access_graph`, so the toggle the docs handed operators also
    /// took away a boot diagnostic.
    ///
    /// **Proving it needs a strict extension.** A directive that is a *prefix*
    /// of the event's target must silence it; two targets that merely share the
    /// `nest_rs::` root prove nothing, because they are silenced apart under
    /// either matching rule — which is what the previous version of this test
    /// compared, so it could not fail on the behaviour it names. `nest_rs::op`
    /// is deliberately not a target anything declares: it exists to be a strict
    /// prefix of one that is.
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
             through, and the `filters` join would be checking a property nothing has",
        );
    }

    /// And the documented family toggle silences the family and **nothing
    /// else** — the other half, and the one an operator relies on.
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

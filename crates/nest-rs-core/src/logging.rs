//! Baseline console logging, installed at boot when no global `tracing`
//! subscriber is set; an observability stack installed in `main` first is left
//! in place. Configuration is env-only, read by [`LogSettings`]:
//!
//! - `<PREFIX>_LOG` (falling back to `RUST_LOG`) — `EnvFilter` directives,
//!   default `info`.
//! - `<PREFIX>_LOG_FORMAT` — `text` or `json`; defaults by build profile
//!   (text in debug, JSON in release).
//! - `<PREFIX>_LOG_SOURCE_LOCATION` — append the emitting `file:line` to each
//!   event; off by default (it leaks source paths).
//!
//! Unset or blank is the default; a value that does not parse aborts the boot,
//! naming the variable and never quoting the value. The same variables drive
//! the console layer of `nest-rs-opentelemetry`. Behind the default `logging`
//! cargo feature.

use std::borrow::Cow;
use std::ffi::OsString;
use std::fmt::{self, Write as _};
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::field::{Field, Visit};
use tracing::{Dispatch, Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::field::VisitOutput;
use tracing_subscriber::filter::{LevelParseError, ParseError};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::{DefaultVisitor, JsonFields, Writer};
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

use crate::env_prefix::EnvPrefix;
use crate::line_safe::{LineSafe, forges};
use crate::request_scope::current_correlation_with;
use crate::stderr_lines::write_stderr_line;
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
}

/// The filter a process logs under when neither `<PREFIX>_LOG` nor `RUST_LOG`
/// sets one.
const DEFAULT_FILTER: &str = "info";

/// The ecosystem's own filter variable, read after `<PREFIX>_LOG`.
const RUST_LOG: &str = "RUST_LOG";

/// The console settings every subscriber the framework ships reads:
/// `<PREFIX>_LOG` (else `RUST_LOG`), `<PREFIX>_LOG_FORMAT` and
/// `<PREFIX>_LOG_SOURCE_LOCATION`.
///
/// They are read from the process environment, which `#[nest_rs::main]` fills
/// with the `.env` cascade before `main` runs. A subscriber builds its filter
/// with `EnvFilter::new(&settings.filter)`, which cannot lose a directive the
/// read already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogSettings {
    /// An `EnvFilter` directive, already checked to parse.
    pub filter: String,
    /// The console format.
    pub format: LogFormat,
    /// Whether each line carries the emitting `file:line`.
    pub source_location: bool,
}

impl LogSettings {
    /// Read and check all three; unset or blank is the default (`info`,
    /// [`LogFormat::by_profile`], `false`), a value that does not parse is
    /// refused naming its variable.
    #[expect(
        clippy::disallowed_methods,
        reason = "the logging bootstrap runs before any config exists"
    )]
    pub fn from_env() -> Result<Self, LogSettingsError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(get: impl Fn(&str) -> Option<OsString>) -> Result<Self, LogSettingsError> {
        // `None` when unset or blank; `Some(None)` when set to a value that is
        // not UTF-8, which no setting parses.
        let set = |name: &str| match get(name)?.into_string() {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(Some(value)),
            Err(_) => Some(None),
        };

        let filter = match [EnvPrefix::var(var::FILTER), RUST_LOG.to_owned()]
            .into_iter()
            .find_map(|name| set(&name).map(|directive| (name, directive)))
        {
            Some((setting, Some(directive))) => {
                Self::check_filter(&directive, &setting)?;
                directive
            }
            Some((setting, None)) => {
                return Err(LogSettingsError::Filter {
                    setting,
                    reason: "the value is not UTF-8",
                });
            }
            None => DEFAULT_FILTER.to_owned(),
        };

        let format_var = EnvPrefix::var(var::FORMAT);
        let format = match set(&format_var) {
            Some(raw) => raw
                .as_deref()
                .and_then(LogFormat::parse)
                .ok_or(LogSettingsError::Format { var: format_var })?,
            None => LogFormat::by_profile(),
        };

        let location_var = EnvPrefix::var(var::SOURCE_LOCATION);
        let source_location = match set(&location_var) {
            Some(raw) => raw
                .as_deref()
                .and_then(crate::parse_bool)
                .ok_or(LogSettingsError::SourceLocation { var: location_var })?,
            None => false,
        };

        Ok(Self {
            filter,
            format,
            source_location,
        })
    }

    /// Check a directive set in code, naming `setting` — the field that
    /// carries it — in the refusal.
    pub fn check_filter(directive: &str, setting: &str) -> Result<(), LogSettingsError> {
        EnvFilter::try_new(directive)
            .map(drop)
            .map_err(|error| LogSettingsError::Filter {
                setting: setting.to_owned(),
                reason: filter_fault(&error),
            })
    }
}

/// What is wrong with a directive, said by its kind: `ParseError`'s own
/// sentence quotes a level or a field pattern it could not read.
fn filter_fault(error: &ParseError) -> &'static str {
    match std::error::Error::source(error) {
        None => {
            "a directive is malformed (each is target[span{field=value}]=level, comma-separated)"
        }
        Some(cause) if cause.is::<LevelParseError>() => {
            "a level is not one of off, error, warn, info, debug, trace"
        }
        Some(_) => "a field filter does not parse",
    }
}

/// A console setting that does not parse; it names the setting, never its
/// value.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LogSettingsError {
    /// `<PREFIX>_LOG`, `RUST_LOG` or a filter set in code is not an
    /// `EnvFilter` directive.
    #[error("{setting} is not a log filter: {reason}")]
    Filter {
        /// The variable or the field carrying the directive.
        setting: String,
        /// What is wrong with it, by kind.
        reason: &'static str,
    },
    /// `<PREFIX>_LOG_FORMAT` is neither `text` nor `json`.
    #[error("{var} must be `text` or `json`")]
    Format {
        /// The variable.
        var: String,
    },
    /// `<PREFIX>_LOG_SOURCE_LOCATION` is not a boolean.
    #[error("{var} must be a boolean: true/false, 1/0, yes/no or on/off")]
    SourceLocation {
        /// The variable.
        var: String,
    },
}

/// How many lines the console's queue holds under `#[nest_rs::main]`: a line
/// that finds it full is dropped and counted. A few hundred bytes a line keeps
/// it to single-digit MiB.
pub const CONSOLE_QUEUE_LINES: usize = 16_384;

/// How long the console's last lines may hold an exit `#[nest_rs::main]`
/// controls — the last bounded step of the way down, which
/// [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT) tabulates.
///
/// What a stdout that stopped taking lines still holds past it is lost, and
/// said in one line on stderr.
pub const CONSOLE_FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

/// How often the lines the console dropped are counted aloud.
const DROP_REPORT_PERIOD: Duration = Duration::from_secs(1);

/// How long the console's writer, woken by a line, lets the lines filed after
/// it gather before writing them: a busy process pays one wake and one write a
/// batch rather than a line.
const BATCH_WINDOW: Duration = Duration::from_millis(2);

/// The bytes a written batch's buffer keeps for the next: a burst's larger one
/// is given back.
const BATCH_BYTES_KEPT: usize = 1 << 20;

/// The console's writer and line formats, and the one way a subscriber becomes
/// the process's global one: the kernel's, reached by the framework's own
/// subscribers through the crate's `__private`, and installed for anyone else
/// by [`install_console`]. They speak `tracing_subscriber`'s traits, a private
/// dependency's, so they are not API.
pub(crate) mod __private {
    use tracing::Dispatch;
    use tracing_subscriber::util::SubscriberInitExt;

    /// The process's console, the writer every subscriber the framework ships
    /// writes its lines through.
    ///
    /// Under `#[nest_rs::main]` it is stdout written in batches by one thread
    /// of its own (`nestrs-log`) through a queue of
    /// [`CONSOLE_QUEUE_LINES`](crate::logging::CONSOLE_QUEUE_LINES): a line that
    /// finds the queue full — a stdout nobody drains — is dropped and counted,
    /// never waited for, so no task stalls on the console. Once a second, while
    /// lines were dropped, one `warn` on [`APP`](crate::target::APP) says how
    /// many since the process started (`dropped`), and every exit
    /// `#[nest_rs::main]` controls waits until each line queued before it is
    /// written, within
    /// [`CONSOLE_FLUSH_TIMEOUT`](crate::logging::CONSOLE_FLUSH_TIMEOUT).
    /// Elsewhere — a test binary, a process that runs an `App` without it — it
    /// is stdout, written synchronously, so nothing queued is lost at an exit
    /// the kernel does not see.
    #[derive(Clone)]
    pub struct ConsoleWriter(pub(in crate::logging) Option<&'static super::Queue>);

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
        pub(in crate::logging) source_location: bool,
    }

    /// The console format for **JSON** output — [`TextFormat`]'s rule, serialized.
    ///
    /// `trace_id`, `span_id`, `trace_flags` and `actor_id` are top-level keys; the
    /// others are `tracing-subscriber`'s own — `timestamp`, `level`, `fields`,
    /// `target`, `filename`, `line_number`.
    #[derive(Clone, Copy, Debug)]
    pub struct JsonFormat {
        pub(in crate::logging) source_location: bool,
    }

    /// Install `subscriber` as the process's global one and bridge `log`
    /// records into it: `false` when the process already has a global
    /// subscriber, which is kept. A `log` logger set before keeps the `log`
    /// records, said once at `warn` through the subscriber just installed.
    pub fn install_global_subscriber(subscriber: impl Into<Dispatch>) -> bool {
        let subscriber = subscriber.into();
        // `try_init` sets the global subscriber before the `log` bridge, and a
        // global subscriber is leaked, never dropped: the weak handle outlives
        // `subscriber` exactly when it was installed, whatever the bridge says.
        let installed = subscriber.downgrade();
        match subscriber.try_init() {
            Ok(()) => true,
            Err(error) if installed.upgrade().is_some() => {
                tracing::warn!(
                    target: crate::target::APP,
                    error = %crate::error_message(&error),
                    "a `log` logger was already set: `log` records keep it, and only `tracing` \
                     events reach the subscriber installed",
                );
                true
            }
            Err(_) => false,
        }
    }
}

use __private::{ConsoleWriter, JsonFormat, TextFormat};

/// The process's console queue, opened on first use; `None` writes stdout
/// directly.
static CONSOLE: OnceLock<Option<Arc<Queue>>> = OnceLock::new();

impl ConsoleWriter {
    /// The process's writer; under `#[nest_rs::main]` its queue and its
    /// thread start on first use.
    pub fn get() -> Self {
        Self(CONSOLE.get_or_init(open).as_deref())
    }

    fn queued(&self) -> bool {
        self.0.is_some()
    }
}

impl fmt::Debug for ConsoleWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = if self.queued() { "queued" } else { "direct" };
        f.debug_tuple("ConsoleWriter").field(&mode).finish()
    }
}

impl<'a> MakeWriter<'a> for ConsoleWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl io::Write for ConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0 {
            Some(queue) => {
                queue.push(buf);
                Ok(buf.len())
            }
            None => io::stdout().write(buf),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self.0 {
            Some(queue) => {
                queue.push(buf);
                Ok(())
            }
            None => io::stdout().write_all(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0 {
            Some(_) => Ok(()),
            None => io::stdout().flush(),
        }
    }
}

/// The console's queue under `#[nest_rs::main]`, shared by every line filed,
/// its writer thread and the exits waiting on it.
struct Queue {
    state: Mutex<Queued>,
    /// Wakes the writer waiting on an empty queue, or gathering a batch an
    /// exit waits for.
    arrived: Condvar,
    /// Wakes an exit waiting on the writer.
    wrote: Condvar,
    capacity: usize,
    /// How long a batch gathers once its first line woke the writer.
    window: Duration,
}

/// Lines end to end, as the writer writes them: in one call.
#[derive(Default)]
struct Batch {
    bytes: Vec<u8>,
    /// Where each line ends in `bytes`.
    ends: Vec<usize>,
}

impl Batch {
    fn len(&self) -> usize {
        self.ends.len()
    }

    fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    fn push(&mut self, line: &[u8]) {
        self.bytes.extend_from_slice(line);
        self.ends.push(self.bytes.len());
    }

    /// Write every line to `out`; past a refusal, how many lines it did not
    /// take whole.
    fn write_to(&self, out: &mut impl io::Write) -> Result<(), (usize, io::Error)> {
        let mut written = 0;
        while written < self.bytes.len() {
            match out.write(&self.bytes[written..]) {
                Ok(0) => return Err((self.left_after(written), io::ErrorKind::WriteZero.into())),
                Ok(taken) => written += taken,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err((self.left_after(written), error)),
            }
        }
        Ok(())
    }

    /// How many lines end past the first `written` bytes.
    fn left_after(&self, written: usize) -> usize {
        self.len() - self.ends.partition_point(|&end| end <= written)
    }

    /// Empty it for the next batch, giving back what a burst grew it to.
    fn clear(&mut self) {
        self.bytes.clear();
        self.ends.clear();
        self.bytes.shrink_to(BATCH_BYTES_KEPT);
    }
}

/// What the queue holds and how far the writer got, under one lock, so an exit
/// waits for exactly the lines queued before it.
#[derive(Default)]
struct Queued {
    lines: Batch,
    /// Every line the queue took.
    taken: u64,
    /// Every line the writer is done with, written or refused.
    done: u64,
    /// Every line dropped: by a full queue, or refused by stdout.
    dropped: u64,
    /// Whether the writer waits for a line.
    writer_idle: bool,
    /// How many exits wait on the writer.
    exits_waiting: usize,
}

impl Queue {
    fn new(capacity: usize, window: Duration) -> Self {
        Self {
            state: Mutex::default(),
            arrived: Condvar::new(),
            wrote: Condvar::new(),
            capacity,
            window,
        }
    }

    fn state(&self) -> MutexGuard<'_, Queued> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue `line`, or drop and count it when the queue is full: it never
    /// waits on stdout.
    fn push(&self, line: &[u8]) {
        let mut state = self.state();
        if state.lines.len() >= self.capacity {
            state.dropped += 1;
            return;
        }
        state.lines.push(line);
        state.taken += 1;
        let wake = std::mem::take(&mut state.writer_idle);
        drop(state);
        if wake {
            self.arrived.notify_one();
        }
    }

    /// Move every queued line into the empty `batch`. On an empty queue, wait
    /// for a line — at most `idle` when given —, then let the lines after it
    /// gather for the queue's window, unless an exit waits for them.
    fn take(&self, batch: &mut Batch, idle: Option<Duration>) {
        let mut state = self.state();
        if state.lines.is_empty() && idle != Some(Duration::ZERO) {
            state.writer_idle = true;
            state = match idle {
                Some(idle) => {
                    self.arrived
                        .wait_timeout(state, idle)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .arrived
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
            state.writer_idle = false;
            let gathered = Instant::now() + self.window;
            while !state.lines.is_empty() && state.exits_waiting == 0 {
                let left = gathered.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                state = self
                    .arrived
                    .wait_timeout(state, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        }
        std::mem::swap(&mut state.lines, batch);
    }

    /// Count the `done` lines the writer handled, `refused` of them by stdout,
    /// for the exits waiting on them. Returns the lines dropped so far.
    fn settle(&self, done: u64, refused: u64) -> u64 {
        let mut state = self.state();
        state.done += done;
        state.dropped += refused;
        if done > 0 && state.exits_waiting > 0 {
            self.wrote.notify_all();
        }
        state.dropped
    }

    /// The writer thread: write every queued batch to `out`, in order, and once
    /// a period, while lines were dropped, file how many. The first line `out`
    /// refuses is also said on stderr, since the count travels on `out`.
    fn write_out(&self, mut out: impl io::Write) {
        let mut batch = Batch::default();
        let (mut dropped, mut said) = (0, 0);
        let mut refusal_said = false;
        let mut report_at = Instant::now() + DROP_REPORT_PERIOD;
        loop {
            let idle =
                (dropped > said).then(|| report_at.saturating_duration_since(Instant::now()));
            self.take(&mut batch, idle);
            let refused = batch
                .write_to(&mut out)
                .err()
                .map_or(0, |(refused, error)| {
                    if !refusal_said {
                        refusal_said = true;
                        write_stderr_line(format_args!(
                            "nestrs: stdout refused a console line, so each it refuses is dropped \
                         and counted: {}",
                            LineSafe(&crate::error_message(&error))
                        ));
                    }
                    refused
                });
            dropped = self.settle(batch.len() as u64, refused as u64);
            batch.clear();
            if Instant::now() >= report_at {
                report_at = Instant::now() + DROP_REPORT_PERIOD;
                if dropped > said {
                    said = dropped;
                    tracing::warn!(
                        target: crate::target::APP,
                        dropped,
                        "log lines dropped while the console was stalled",
                    );
                }
            }
        }
    }

    /// Wait until the writer is done with every line queued before this call,
    /// at most `bound`; whether it was.
    fn written_within(&self, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        let mut state = self.state();
        let queued = state.taken;
        state.exits_waiting += 1;
        // A batch still gathering is written at once.
        self.arrived.notify_one();
        let written = loop {
            if state.done >= queued {
                break true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break false;
            }
            state = self
                .wrote
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        state.exits_waiting -= 1;
        written
    }
}

fn open() -> Option<Arc<Queue>> {
    if !crate::way_down::under_main() {
        return None;
    }
    started(
        |writer| {
            std::thread::Builder::new()
                .name("nestrs-log".to_owned())
                .spawn(move || writer.write_out(io::stdout()))
                .map(drop)
        },
        write_stderr_line,
    )
}

/// The console's queue, once `spawn` started the thread writing it out; or
/// `None` — stdout written directly — when it could not, which `say` reports.
fn started(
    spawn: impl FnOnce(Arc<Queue>) -> io::Result<()>,
    say: impl FnOnce(fmt::Arguments<'_>),
) -> Option<Arc<Queue>> {
    let queue = Arc::new(Queue::new(CONSOLE_QUEUE_LINES, BATCH_WINDOW));
    match spawn(Arc::clone(&queue)) {
        Ok(()) => Some(queue),
        Err(error) => {
            say(format_args!(
                "nestrs: the console queue could not start, so stdout is written directly: {}",
                LineSafe(&crate::error_message(&error))
            ));
            None
        }
    }
}

/// Wait until every line the console queued so far is written, at most
/// [`CONSOLE_FLUSH_TIMEOUT`]: the exits `#[nest_rs::main]` controls call it
/// last, and a line filed after it is not waited for. A line about the console
/// itself goes to stderr, which its queue never holds.
pub(crate) fn flush_console() {
    let Some(queue) = CONSOLE.get().and_then(Option::as_deref) else {
        return;
    };
    if !queue.written_within(CONSOLE_FLUSH_TIMEOUT) {
        write_stderr_line(format_args!(
            "nestrs: the console's last lines were abandoned after {} ms: stdout stopped taking them",
            CONSOLE_FLUSH_TIMEOUT.as_millis()
        ));
    }
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
    current_correlation_with(write).unwrap_or(Ok(()))
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

/// Install the fallback console subscriber unless one is already set.
pub(crate) fn init_fallback() -> Result<()> {
    if crate::process_start::subscriber_set() {
        return Ok(());
    }
    install_console(&LogSettings::from_env()?);
    Ok(())
}

/// Install the kernel's console subscriber as the process's global one, as
/// `settings` describe it — their filter, their format and their source
/// location — writing through the process's console: the subscriber an `App`
/// installs when `main` installed none, for a binary that runs no `App` (a
/// migration runner, a seed).
///
/// `false` when the process already has a global subscriber, which is kept.
/// `log` records reach the console too, unless a `log` logger was set before,
/// which keeps them: the console then says so once, at `warn`.
///
/// ```no_run
/// use nest_rs_core::logging::{LogSettings, install_console};
///
/// #[nest_rs_core::main]
/// async fn main() -> anyhow::Result<()> {
///     install_console(&LogSettings::from_env()?);
///     tracing::info!("migrating");
///     Ok(())
/// }
/// ```
pub fn install_console(settings: &LogSettings) -> bool {
    let builder = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&settings.filter))
        .with_writer(ConsoleWriter::get());
    __private::install_global_subscriber(match settings.format {
        LogFormat::Text => Dispatch::new(
            builder
                .event_format(TextFormat::new(settings.source_location))
                .finish(),
        ),
        // `.json()` sets the field formatter only; the envelope is `JsonFormat`'s.
        LogFormat::Json => Dispatch::new(
            builder
                .json()
                .event_format(JsonFormat::new(settings.source_location))
                .finish(),
        ),
    })
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

    /// The settings `set` describes, every other variable unset.
    fn settings_under(set: &[(String, &str)]) -> Result<LogSettings, LogSettingsError> {
        LogSettings::from_lookup(|name| {
            set.iter()
                .find(|(variable, _)| variable == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[cfg(unix)]
    #[test]
    fn a_value_that_is_not_utf8_is_refused_naming_its_variable() {
        use std::os::unix::ffi::OsStringExt as _;

        for variable in [
            EnvPrefix::var(var::FILTER),
            RUST_LOG.to_owned(),
            EnvPrefix::var(var::FORMAT),
            EnvPrefix::var(var::SOURCE_LOCATION),
        ] {
            let refused = LogSettings::from_lookup(|name| {
                (name == variable).then(|| OsString::from_vec(b"info\xff".to_vec()))
            })
            .expect_err("a value that is not UTF-8 is set, and parses as nothing");
            let said = refused.to_string();
            assert!(said.starts_with(&format!("{variable} ")), "{said}");
            assert!(!said.contains("info"), "the value is never quoted: {said}");
        }
    }

    #[test]
    fn every_setting_unset_or_blank_is_its_default() {
        let defaults = LogSettings {
            filter: "info".to_owned(),
            format: LogFormat::by_profile(),
            source_location: false,
        };
        assert_eq!(settings_under(&[]).expect("the defaults"), defaults);
        let blank = [
            (EnvPrefix::var(var::FILTER), " "),
            (RUST_LOG.to_owned(), ""),
            (EnvPrefix::var(var::FORMAT), ""),
            (EnvPrefix::var(var::SOURCE_LOCATION), "  "),
        ];
        assert_eq!(settings_under(&blank).expect("the defaults"), defaults);
    }

    #[test]
    fn the_format_reads_its_two_names_trimmed_and_in_any_case() {
        for (raw, format) in [
            (" JSON ", LogFormat::Json),
            ("json", LogFormat::Json),
            ("text", LogFormat::Text),
            ("Text", LogFormat::Text),
        ] {
            let read = settings_under(&[(EnvPrefix::var(var::FORMAT), raw)]).expect(raw);
            assert_eq!(read.format, format, "{raw:?}");
        }
    }

    #[test]
    fn a_format_that_does_not_parse_is_refused_naming_the_variable_and_not_the_value() {
        let variable = EnvPrefix::var(var::FORMAT);
        let refused = settings_under(&[(variable.clone(), "jsno")]).expect_err("not a format");

        assert!(
            matches!(&refused, LogSettingsError::Format { var } if *var == variable),
            "{refused:?}"
        );
        let said = refused.to_string();
        assert!(said.contains(&variable), "{said}");
        assert!(!said.contains("jsno"), "{said}");
    }

    #[test]
    fn a_source_location_that_is_not_a_boolean_is_refused_naming_the_variable() {
        let variable = EnvPrefix::var(var::SOURCE_LOCATION);
        let refused = settings_under(&[(variable.clone(), "ture")]).expect_err("not a boolean");

        assert!(
            matches!(&refused, LogSettingsError::SourceLocation { var } if *var == variable),
            "{refused:?}"
        );
        let said = refused.to_string();
        assert!(said.contains(&variable) && !said.contains("ture"), "{said}");
        assert!(
            settings_under(&[(variable, "on")])
                .expect("a boolean")
                .source_location
        );
    }

    #[test]
    fn rust_log_answers_when_the_prefixed_filter_is_unset_and_is_named_when_refused() {
        let read = settings_under(&[(RUST_LOG.to_owned(), "debug")]).expect("a filter");
        assert_eq!(read.filter, "debug");

        let first = settings_under(&[
            (EnvPrefix::var(var::FILTER), "warn"),
            (RUST_LOG.to_owned(), "debug"),
        ])
        .expect("a filter");
        assert_eq!(first.filter, "warn", "the prefixed variable wins");

        let refused =
            settings_under(&[(RUST_LOG.to_owned(), "billing=notalevel")]).expect_err("refused");
        assert!(
            matches!(&refused, LogSettingsError::Filter { setting, .. } if setting == RUST_LOG),
            "{refused:?}"
        );
    }

    #[test]
    fn a_wrong_level_is_said_by_its_kind_without_the_directive() {
        let refused = LogSettings::check_filter("billing=notalevel", "f").expect_err("a level");
        let said = refused.to_string();

        assert!(said.starts_with("f is not a log filter: "), "{said}");
        assert!(said.contains("a level is not one of"), "{said}");
        assert!(!said.contains("billing"), "{said}");
        assert!(!said.contains("notalevel"), "{said}");
    }

    #[test]
    fn a_malformed_directive_is_said_by_its_kind() {
        let said = LogSettings::check_filter("billing[charge", "f")
            .expect_err("an unclosed span")
            .to_string();
        assert!(said.contains("a directive is malformed"), "{said}");
        assert!(!said.contains("charge"), "{said}");
    }

    #[test]
    fn a_field_pattern_that_does_not_compile_is_said_by_its_kind_without_the_pattern() {
        let said = LogSettings::check_filter("billing[{card=sk_live(}]=debug", "f")
            .expect_err("a field pattern")
            .to_string();
        assert!(said.contains("a field filter does not parse"), "{said}");
        assert!(!said.contains("sk_live"), "{said}");
    }

    #[test]
    fn a_process_outside_the_kernel_s_main_writes_its_console_directly() {
        let console = ConsoleWriter::get();

        assert!(!console.queued(), "a test binary is never queued");
        assert_eq!(format!("{console:?}"), r#"ConsoleWriter("direct")"#);
        flush_console();
    }

    /// A stdout that takes nothing until its gate is released, then every
    /// write, each recorded whole; it says when its first write began.
    struct GatedStdout {
        gate: Option<std::sync::mpsc::Receiver<()>>,
        began: Option<std::sync::mpsc::Sender<()>>,
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl io::Write for GatedStdout {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(began) = self.began.take() {
                began.send(()).unwrap_or_default();
            }
            if let Some(gate) = self.gate.take() {
                gate.recv().unwrap_or_default();
            }
            self.lines
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(String::from_utf8_lossy(buf).into_owned());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A stdout whose reader went away.
    struct ClosedStdout;

    impl io::Write for ClosedStdout {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A queue of `capacity` lines, its writer thread writing to `out`.
    fn queue_writing_to(capacity: usize, out: impl io::Write + Send + 'static) -> Arc<Queue> {
        writing_to(Queue::new(capacity, BATCH_WINDOW), out)
    }

    /// `queue`, its writer thread writing to `out`.
    fn writing_to(queue: Queue, out: impl io::Write + Send + 'static) -> Arc<Queue> {
        let queue = Arc::new(queue);
        let writer = Arc::clone(&queue);
        std::thread::spawn(move || writer.write_out(out));
        queue
    }

    #[test]
    fn a_writer_thread_that_cannot_start_leaves_stdout_written_directly_and_says_so() {
        let mut said = String::new();
        let queue = started(
            |_| Err(io::Error::other("no thread\nforged=line")),
            |line| said = line.to_string(),
        );
        assert!(queue.is_none(), "the console falls back to a direct stdout");
        assert!(
            said.starts_with(
                "nestrs: the console queue could not start, so stdout is written directly: "
            ),
            "{said}"
        );
        assert!(!said.contains('\n'), "one line, escaped: {said:?}");

        let mut asked = false;
        let queue = started(
            |_| {
                asked = true;
                Ok(())
            },
            |_| panic!("nothing to say"),
        );
        assert!(queue.is_some() && asked, "a started writer keeps the queue");
    }

    #[test]
    fn a_line_that_finds_the_queue_full_is_dropped_and_counted() {
        let queue = Queue::new(2, BATCH_WINDOW);

        for line in ["one\n", "two\n", "three\n"] {
            queue.push(line.as_bytes());
        }

        let state = queue.state();
        assert_eq!(state.lines.bytes, b"one\ntwo\n");
        assert_eq!(state.lines.len(), 2);
        assert_eq!((state.taken, state.dropped), (2, 1));
    }

    #[test]
    fn the_lines_after_the_one_that_woke_the_writer_leave_with_it_once_an_exit_waits() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let queue = writing_to(
            Queue::new(8, Duration::from_secs(600)),
            GatedStdout {
                gate: None,
                began: None,
                lines: Arc::clone(&written),
            },
        );
        let idle_by = Instant::now() + Duration::from_secs(10);
        while !queue.state().writer_idle {
            assert!(Instant::now() < idle_by, "the writer waits for a line");
            std::thread::sleep(Duration::from_millis(1));
        }
        queue.push(b"one\n");
        // Time enough for a writer that would not gather to write `one` alone.
        std::thread::sleep(Duration::from_millis(50));
        queue.push(b"two\n");
        queue.push(b"three\n");
        let exit = Instant::now();

        assert!(queue.written_within(Duration::from_secs(10)));
        assert!(
            exit.elapsed() < Duration::from_secs(5),
            "the exit cut the window short: {:?}",
            exit.elapsed()
        );
        let written = written.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(written, ["one\ntwo\nthree\n"]);
    }

    /// A stdout that takes `room` bytes, then refuses.
    struct FillingStdout {
        room: usize,
    }

    impl io::Write for FillingStdout {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.room == 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let taken = buf.len().min(self.room).min(3);
            self.room -= taken;
            Ok(taken)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_batch_stdout_refuses_midway_counts_each_line_it_did_not_take_whole() {
        let mut batch = Batch::default();
        for line in ["one\n", "two\n", "three\n"] {
            batch.push(line.as_bytes());
        }

        let whole = batch.write_to(&mut FillingStdout { room: 4 });
        let cut = batch.write_to(&mut FillingStdout { room: 6 });
        let all = batch.write_to(&mut FillingStdout { room: 64 });

        assert_eq!(whole.map_err(|(left, _)| left), Err(2), "`one` went whole");
        assert_eq!(cut.map_err(|(left, _)| left), Err(2), "`two` was cut");
        assert_eq!(all.map_err(|(left, _)| left), Ok(()));
    }

    #[test]
    fn an_exit_waits_until_every_line_queued_before_it_is_written() {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let written = Arc::new(Mutex::new(Vec::new()));
        let queue = queue_writing_to(
            8,
            GatedStdout {
                gate: Some(gate),
                began: None,
                lines: Arc::clone(&written),
            },
        );
        for line in ["one\n", "two\n", "three\n"] {
            queue.push(line.as_bytes());
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(release);
        });

        assert!(
            queue.written_within(Duration::from_secs(10)),
            "stdout took every line inside the bound"
        );
        let written = written.lock().unwrap_or_else(|e| e.into_inner()).concat();
        assert_eq!(written, "one\ntwo\nthree\n");
    }

    #[test]
    fn the_lines_filed_while_stdout_takes_a_write_leave_in_one_write() {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let (began, writing) = std::sync::mpsc::channel::<()>();
        let written = Arc::new(Mutex::new(Vec::new()));
        let queue = queue_writing_to(
            8,
            GatedStdout {
                gate: Some(gate),
                began: Some(began),
                lines: Arc::clone(&written),
            },
        );
        queue.push(b"one\n");
        writing
            .recv_timeout(Duration::from_secs(10))
            .expect("the writer took the first line");
        for line in ["two\n", "three\n", "four\n"] {
            queue.push(line.as_bytes());
        }
        drop(release);

        assert!(queue.written_within(Duration::from_secs(10)));
        let written = written.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(written, ["one\n", "two\nthree\nfour\n"]);
    }

    #[test]
    fn an_exit_stops_waiting_at_its_bound() {
        let (_never_released, gate) = std::sync::mpsc::channel::<()>();
        let queue = queue_writing_to(
            8,
            GatedStdout {
                gate: Some(gate),
                began: None,
                lines: Arc::default(),
            },
        );
        queue.push(b"held\n");
        let bound = Duration::from_millis(200);
        let started = Instant::now();

        assert!(!queue.written_within(bound), "stdout never took the line");
        let took = started.elapsed();
        assert!(took >= bound && took < Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn a_line_stdout_refuses_is_dropped_and_counted_without_holding_an_exit() {
        let queue = queue_writing_to(8, ClosedStdout);
        queue.push(b"one\n");
        queue.push(b"two\n");

        assert!(queue.written_within(Duration::from_secs(10)));
        let state = queue.state();
        assert_eq!((state.taken, state.done, state.dropped), (2, 2, 2));
    }

    #[test]
    fn a_valid_directive_passes_the_check() {
        LogSettings::check_filter("debug,hyper=warn,billing[charge{card}]=trace", "f")
            .expect("valid");
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

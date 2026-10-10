//! A panic, said once and without its value: [`contain`] is how a unit of work
//! catches its own unwind, and the process hook `#[nest_rs::main]` installs
//! files every panic no unit contains.

use std::any::Any;
use std::backtrace::{Backtrace, BacktraceStatus};
use std::collections::VecDeque;
use std::future::Future;
use std::io::Write as _;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::sync::{Mutex, PoisonError};

use futures_util::FutureExt as _;

/// The field name a contained panic is logged under.
pub const FIELD: &str = "panic";

/// The field name where a panic happened (`file:line:col`) is logged under.
pub const LOCATION_FIELD: &str = "panic_location";

/// Best-effort message from a caught panic payload (`&str` or `String`), with
/// serde's sentences redacted ([`DecodeError::redact`](crate::DecodeError::redact)):
/// `.unwrap()` on a failed decode puts the value into the payload.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    crate::DecodeError::redact(message, None).into_owned()
}

/// Run `fut`, catching its unwind; the process hook stays quiet for a panic
/// inside it, since the unit that called this reports it.
///
/// ```
/// # #[nest_rs_core::main]
/// # async fn main() {
/// let unwound = nest_rs_core::panic::contain(async { panic!("boom") }).await;
/// let payload = unwound.expect_err("it panicked");
/// nest_rs_core::contained_panic!(
///     target: "nest_rs::fixture",
///     payload.as_ref(),
///     "unit panicked — the next one runs",
/// );
/// # }
/// ```
pub async fn contain<F: Future>(fut: F) -> Result<F::Output, Box<dyn Any + Send>> {
    CONTAINED
        .scope((), AssertUnwindSafe(fut).catch_unwind())
        .await
}

tokio::task_local! {
    /// Set while a [`contain`]ed future is polled: a task-local, so a task it
    /// spawns is outside it.
    static CONTAINED: ();
}

/// Log a contained panic: at `error`, on `target`, with the payload rendered by
/// [`panic_message`] under [`FIELD`], where it happened under
/// [`LOCATION_FIELD`] when the process hook saw it, then the seam's own fields.
///
/// ```
/// # const TARGET: &str = "nest_rs::fixture";
/// let payload = std::panic::catch_unwind(|| panic!("boom")).expect_err("it panics");
/// nest_rs_core::contained_panic!(
///     target: TARGET,
///     payload.as_ref(),
///     "listener panicked — dispatch continues",
///     listener = "notify",
/// );
/// ```
///
/// Pass `payload.as_ref()` or `&*payload`, never `&payload`: a `Box<dyn Any +
/// Send>` is itself `Any`, so a borrow of the box unsizes to the box and every
/// downcast misses.
#[macro_export]
macro_rules! contained_panic {
    (target: $target:expr, $payload:expr, $message:literal $(, $($field:tt)*)?) => {{
        let unwound = $crate::__private::unwound($payload);
        $crate::tracing::error!(
            target: $target,
            // `tracing` does not parse a `{ CONST }` field written first after `target:`.
            message = ::core::format_args!($message),
            { $crate::panic::FIELD } = %unwound.message,
            { $crate::panic::LOCATION_FIELD } = unwound.location.as_deref(),
            $($($field)*)?
        )
    }};
}

/// The panic hook `#[nest_rs::main]` installs: a panic inside a [`contain`]ed
/// unit is that unit's to file, and any other is one `error` on
/// [`APP`](crate::target::APP) — or one line on stderr while no subscriber
/// can take it. Never the payload unredacted.
pub(crate) fn hook(info: &PanicHookInfo<'_>) {
    let message = panic_message(info.payload());
    let location = info
        .location()
        .map_or_else(|| "<unknown>".to_owned(), ToString::to_string);
    remember(&message, &location);
    match report(CONTAINED.try_with(|()| ()).is_ok(), subscribed()) {
        Report::ByTheUnit => {}
        Report::Event => {
            let thread = std::thread::current();
            let backtrace = Backtrace::capture();
            let backtrace = (backtrace.status() == BacktraceStatus::Captured)
                .then(|| tracing::field::display(backtrace));
            tracing::error!(
                target: crate::target::APP,
                thread = thread.name().unwrap_or("<unnamed>"),
                { FIELD } = %message,
                { LOCATION_FIELD } = %location,
                backtrace,
                "panicked where no unit of work contains it",
            );
        }
        Report::Stderr => {
            let line = format!("nestrs: panicked at {location}: {}\n", one_line(&message));
            #[expect(
                clippy::let_underscore_must_use,
                reason = "stderr is the last channel: a write it refuses has nowhere left to be said"
            )]
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
    }
}

/// Who says a panic.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    /// The unit that contains it, through [`contained_panic!`](crate::contained_panic).
    ByTheUnit,
    /// The hook, as one event.
    Event,
    /// The hook, as one line on stderr: no event would reach anyone.
    Stderr,
}

fn report(contained: bool, subscribed: bool) -> Report {
    match (contained, subscribed) {
        (_, false) => Report::Stderr,
        (true, true) => Report::ByTheUnit,
        (false, true) => Report::Event,
    }
}

/// Whether an event emitted on this thread now reaches a subscriber: `false`
/// before one is installed, and inside a scoped one's own dispatch.
fn subscribed() -> bool {
    !tracing::dispatcher::get_default(|dispatch| dispatch.is::<tracing::subscriber::NoSubscriber>())
}

/// `message` on one line, its control characters escaped.
fn one_line(message: &str) -> String {
    let mut line = String::with_capacity(message.len());
    for c in message.chars() {
        if c.is_control() {
            line.extend(c.escape_default());
        } else {
            line.push(c);
        }
    }
    line
}

/// How many recent panics keep where they happened; one a line never asks
/// about (a unit that resumed the unwind) is forgotten past it.
const RECENT_PANICS: usize = 16;

/// Where recent panics happened, newest last, for the line that contains one
/// to say. Process-wide: a unit may file its line on another thread than the
/// one it panicked on, past an `.await` or across a task.
static RECENT: Mutex<VecDeque<Recent>> = Mutex::new(VecDeque::new());

struct Recent {
    message: String,
    location: String,
}

fn remember(message: &str, location: &str) {
    let mut recent = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
    if recent.len() == RECENT_PANICS {
        recent.pop_front();
    }
    recent.push_back(Recent {
        message: message.to_owned(),
        location: location.to_owned(),
    });
}

/// Where the newest panic saying `message` happened, forgotten once read.
fn recall(message: &str) -> Option<String> {
    let mut recent = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
    let at = recent.iter().rposition(|panic| panic.message == message)?;
    recent.remove(at).map(|panic| panic.location)
}

pub(crate) mod __private {
    use std::any::Any;

    /// A contained panic, as its line says it.
    pub struct Unwound {
        /// [`panic_message`](super::panic_message)'s rendering.
        pub message: String,
        /// Where it happened, when the process hook saw it.
        pub location: Option<String>,
    }

    /// What [`contained_panic!`](crate::contained_panic) files for `payload`.
    pub fn unwound(payload: &(dyn Any + Send)) -> Unwound {
        let message = super::panic_message(payload);
        let location = super::recall(&message);
        Unwound { message, location }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_two_shapes_panic_produces_and_names_the_third() {
        let literal: Box<dyn Any + Send> = Box::new("deliberate panic");
        assert_eq!(panic_message(literal.as_ref()), "deliberate panic");

        let formatted: Box<dyn Any + Send> = Box::new(format!("panic for {}", "boom"));
        assert_eq!(panic_message(formatted.as_ref()), "panic for boom");

        let opaque: Box<dyn Any + Send> = Box::new(42u8);
        assert_eq!(panic_message(opaque.as_ref()), "<non-string panic payload>");
    }

    #[test]
    fn a_panic_over_a_decode_failure_is_said_without_its_value() {
        let payload = std::panic::catch_unwind(|| {
            serde_json::from_str::<u64>(r#""sk_live_51HsecretTOKEN""#).unwrap()
        })
        .expect_err("a secret is not a number");
        let message = panic_message(payload.as_ref());
        assert!(!message.contains("sk_live"), "{message}");
        assert_eq!(
            message,
            "called `Result::unwrap()` on an `Err` value: Error(\"invalid type: a string, \
             expected u64\", line: 1, column: 24)"
        );
    }

    #[test]
    fn a_panic_is_said_by_its_unit_by_the_hook_or_on_stderr_when_nothing_listens() {
        assert_eq!(report(true, true), Report::ByTheUnit);
        assert_eq!(report(false, true), Report::Event);
        assert_eq!(report(true, false), Report::Stderr);
        assert_eq!(report(false, false), Report::Stderr);
    }

    #[test]
    fn an_event_reaches_a_subscriber_only_once_one_is_installed() {
        assert!(!subscribed(), "no subscriber yet");
        let _capture = nest_rs_testing::LogCapture::install();
        assert!(subscribed());
    }

    #[test]
    fn the_stderr_line_is_one_line() {
        assert_eq!(one_line("first\nsecond\tthird"), "first\\nsecond\\tthird");
        assert_eq!(one_line("déjà vu"), "déjà vu");
    }

    #[test]
    fn a_location_is_recalled_by_the_panic_that_says_the_same_and_only_once() {
        remember("unrelated", "src/a.rs:1:1");
        remember("deliberate", "src/b.rs:2:2");
        remember("deliberate", "src/c.rs:3:3");

        assert_eq!(recall("deliberate").as_deref(), Some("src/c.rs:3:3"));
        assert_eq!(recall("deliberate").as_deref(), Some("src/b.rs:2:2"));
        assert_eq!(recall("deliberate"), None);
        assert_eq!(recall("unrelated").as_deref(), Some("src/a.rs:1:1"));
    }

    #[test]
    fn only_the_most_recent_panics_keep_where_they_happened() {
        remember("oldest", "src/old.rs:1:1");
        for _ in 0..RECENT_PANICS {
            remember("newer", "src/new.rs:1:1");
        }
        assert_eq!(recall("oldest"), None);
    }
}

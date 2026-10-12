//! The steps `#[nest_rs::main]` runs before the runtime exists, while the
//! process has one thread: what is unsound once another thread may read
//! alongside it — publishing the `.env` cascade into the process environment.

use std::fmt;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use crate::line_safe::LineSafe;

/// A step `#[nest_rs::main]` runs before the runtime exists, sorted by `name`.
pub struct ProcessStart {
    /// Orders the steps; named like a span target (`nest_rs::config::cascade`).
    pub name: &'static str,
    /// The step.
    pub run: fn(),
}

inventory::collect!(ProcessStart);

/// Whether the steps ran under [`StderrLines`], the first default this
/// process set.
static STEPS_SAID_ON_STDERR: AtomicBool = AtomicBool::new(false);

/// Run every step this binary links, by name. No subscriber exists yet, so
/// what a step files at `warn` or `error` is written to stderr.
pub(crate) fn run_all() {
    let mut steps: Vec<&ProcessStart> = inventory::iter::<ProcessStart>().collect();
    steps.sort_by_key(|step| step.name);
    let run = || {
        for step in steps {
            (step.run)();
        }
    };
    if tracing::dispatcher::has_been_set() {
        run();
    } else {
        STEPS_SAID_ON_STDERR.store(true, Ordering::Relaxed);
        tracing::subscriber::with_default(StderrLines, run);
    }
}

/// Whether a subscriber is set, so the kernel's fallback steps aside.
///
/// tracing's `has_been_set` stays true once any default was set, a scoped one
/// long gone included: when the steps' own was the first, the default this
/// thread now reaches is read instead.
#[cfg(feature = "logging")]
pub(crate) fn subscriber_set() -> bool {
    if STEPS_SAID_ON_STDERR.load(Ordering::Relaxed) {
        crate::panic::subscribed()
    } else {
        tracing::dispatcher::has_been_set()
    }
}

/// What a step files before any subscriber exists: each `warn` and `error` as
/// one line on stderr, `nestrs: <message> <field>=<value>…`, escaped as the
/// panic hook's line of that moment is.
pub(crate) struct StderrLines;

impl Subscriber for StderrLines {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event() && *metadata.level() <= Level::WARN
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let text = format!("{}\n", render(event));
        #[expect(
            clippy::let_underscore_must_use,
            reason = "stderr is the last channel: a write it refuses has nowhere left to be said"
        )]
        let _ = std::io::stderr().write_all(text.as_bytes());
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

fn render(event: &Event<'_>) -> String {
    let mut line = Line::default();
    event.record(&mut line);
    format!("nestrs: {}{}", line.message, line.fields)
}

/// An event's message, then its other fields, each value escaped.
#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl Visit for Line {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(&LineSafe(value).to_string());
        } else {
            self.fields
                .push_str(&format!(" {}={:?}", field.name(), LineSafe(value)));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message.push_str(&format!("{:?}", LineSafe(value)));
        } else {
            self.fields
                .push_str(&format!(" {}={:?}", field.name(), LineSafe(value)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;

    /// `render`'s output for what `emit` files, through `StderrLines`' filter.
    #[derive(Default)]
    struct Rendered(Mutex<Vec<String>>);

    impl Subscriber for Rendered {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            StderrLines.enabled(metadata)
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(render(event));
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    fn rendered(emit: impl FnOnce()) -> Vec<String> {
        let lines = Arc::new(Rendered::default());
        tracing::subscriber::with_default(Arc::clone(&lines), emit);
        lines
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    #[test]
    fn a_line_says_the_message_then_its_fields_and_forges_nothing() {
        let lines = rendered(|| {
            tracing::warn!(
                target: "nest_rs::fixture",
                path = %"./.env\nnestrs: forged",
                skipped = 2u64,
                "skipped malformed lines",
            );
        });

        assert_eq!(
            lines,
            [r#"nestrs: skipped malformed lines path=./.env\nnestrs: forged skipped=2"#]
        );
    }

    #[test]
    fn only_a_warn_or_an_error_is_written() {
        let lines = rendered(|| {
            tracing::info!(target: "nest_rs::fixture", "informs");
            tracing::error!(target: "nest_rs::fixture", "fails");
        });

        assert_eq!(lines, ["nestrs: fails"]);
    }
}

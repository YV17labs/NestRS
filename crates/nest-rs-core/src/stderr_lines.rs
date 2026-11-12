//! The lines nestrs writes on stderr, the channel left when no subscriber can
//! carry one: an event filed before any subscriber exists, a panic the hook
//! reports there.

use std::fmt;
use std::io::Write as _;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use crate::line_safe::LineSafe;

/// Write `line` and its newline to stderr in one call, so no other thread's
/// line lands inside it.
pub(crate) fn write_stderr_line(line: fmt::Arguments<'_>) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "stderr is the last channel: a write it refuses has nowhere left to be said"
    )]
    let _ = std::io::stderr().write_all(format!("{line}\n").as_bytes());
}

/// What a step, or a signal before any app, files before any subscriber
/// exists: each `warn` and `error` as one line on stderr,
/// `nestrs: <message> <field>=<value>…`, escaped as the panic hook's line of
/// that moment is.
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
        write_stderr_line(format_args!("{}", render(event)));
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
            self.record_debug(field, &value);
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

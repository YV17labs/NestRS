//! One operation this process asked of another — a statement sent to a
//! database, a command sent to a store — timed by the driver that sent it and
//! handed, once it answered, to whatever records it.
//!
//! The observability stack seeds the recorder at boot; with none seeded,
//! [`is_recorded`] is `false` and a driver times nothing. What an operation
//! carries is the server's: its name and its attributes say which operation
//! and on what, never a value it sent.

use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// The histogram a kind of client operation records its duration in, named as
/// its telemetry conventions name it, and the attributes that key it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientDuration {
    name: &'static str,
    description: &'static str,
    keys: &'static [&'static str],
}

impl ClientDuration {
    /// The histogram `name`, described by `description`, keyed by the
    /// attributes named `keys`.
    pub const fn new(
        name: &'static str,
        description: &'static str,
        keys: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            description,
            keys,
        }
    }

    /// The instrument's name, as the conventions spell it
    /// (`db.client.operation.duration`).
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// What the instrument measures, as the conventions describe it.
    pub const fn description(&self) -> &'static str {
        self.description
    }

    /// The attributes a recorded duration is keyed by; `error.type` keys every
    /// failed one besides.
    pub const fn keys(&self) -> &'static [&'static str] {
        self.keys
    }
}

/// A value one of an operation's attributes holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientValue<'a> {
    /// A string.
    Text(&'a str),
    /// An integer.
    Integer(i64),
}

/// One client operation, as it answered: what it was, when it started and how
/// long it took, the attributes its conventions name, and how it failed.
#[derive(Clone, Copy, Debug)]
pub struct ClientOperation<'a> {
    duration: &'static ClientDuration,
    name: &'a str,
    started: SystemTime,
    elapsed: Duration,
    attributes: &'a [(&'static str, ClientValue<'a>)],
    error_type: Option<&'a str>,
}

impl<'a> ClientOperation<'a> {
    /// The operation `name`, recorded in `duration`, started at `started` and
    /// answered `elapsed` later, carrying `attributes`.
    pub const fn new(
        duration: &'static ClientDuration,
        name: &'a str,
        started: SystemTime,
        elapsed: Duration,
        attributes: &'a [(&'static str, ClientValue<'a>)],
    ) -> Self {
        Self {
            duration,
            name,
            started,
            elapsed,
            attributes,
            error_type: None,
        }
    }

    /// The same operation, failed with the class `error_type` — a status code
    /// its protocol answered, or `_OTHER`.
    pub const fn failed(self, error_type: &'a str) -> Self {
        Self {
            error_type: Some(error_type),
            ..self
        }
    }

    /// The histogram it records in.
    pub const fn duration(&self) -> &'static ClientDuration {
        self.duration
    }

    /// Its span's name, as its conventions build it (`SELECT posts`).
    pub const fn name(&self) -> &'a str {
        self.name
    }

    /// When it was sent.
    pub const fn started(&self) -> SystemTime {
        self.started
    }

    /// How long it took to answer.
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// The attributes its conventions name.
    pub const fn attributes(&self) -> &'a [(&'static str, ClientValue<'a>)] {
        self.attributes
    }

    /// How it failed, if it did.
    pub const fn error_type(&self) -> Option<&'a str> {
        self.error_type
    }

    /// Hand it to the recorder the observability stack seeded; nothing
    /// happens where none was.
    pub fn record(&self) {
        if let Some(recorder) = RECORDER.get() {
            recorder(self);
        }
    }
}

/// What records a client operation once it answered.
type ClientRecorder = fn(&ClientOperation<'_>);

static RECORDER: OnceLock<ClientRecorder> = OnceLock::new();

/// Whether anything records client operations: a driver times none when
/// nothing does.
pub fn is_recorded() -> bool {
    RECORDER.get().is_some()
}

/// `client_operation` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use super::{ClientRecorder, RECORDER};

    /// Seed what records every client operation. Called once, at boot, by the
    /// observability stack — never by an application.
    pub fn set_client_recorder(recorder: ClientRecorder) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "seeded once at boot; a second install keeps the first recorder"
        )]
        let _ = RECORDER.set(recorder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATEMENT: ClientDuration = ClientDuration::new(
        "db.client.operation.duration",
        "Duration of database client operations.",
        &["db.system.name"],
    );

    #[test]
    fn an_operation_is_failed_by_its_class_and_keeps_the_rest() {
        let started = SystemTime::now();
        let attributes = [("db.system.name", ClientValue::Text("postgresql"))];
        let operation = ClientOperation::new(
            &STATEMENT,
            "SELECT posts",
            started,
            Duration::from_millis(3),
            &attributes,
        );
        assert_eq!(operation.error_type(), None);

        let failed = operation.failed("_OTHER");
        assert_eq!(failed.error_type(), Some("_OTHER"));
        assert_eq!(failed.name(), "SELECT posts");
        assert_eq!(failed.started(), started);
        assert_eq!(failed.elapsed(), Duration::from_millis(3));
        assert_eq!(failed.attributes(), &attributes);
        assert_eq!(failed.duration(), &STATEMENT);
    }
}

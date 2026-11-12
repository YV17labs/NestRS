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

use crate::Correlation;
use crate::duration_histogram::{DurationHistogram, MetricAttribute};

/// The database conventions' client duration, which every database driver
/// records its operations in.
pub const DB_CLIENT_OPERATION_DURATION: DurationHistogram = DurationHistogram::new(
    "db.client.operation.duration",
    "Duration of database client operations.",
    &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0],
    &[
        MetricAttribute::bounded(DB_SYSTEM_NAME),
        MetricAttribute::bounded(DB_NAMESPACE),
        MetricAttribute::bounded(DB_OPERATION_NAME),
        MetricAttribute::bounded(DB_COLLECTION_NAME),
        MetricAttribute::bounded(DB_QUERY_SUMMARY),
        MetricAttribute::bounded(SERVER_ADDRESS),
        MetricAttribute::bounded(SERVER_PORT),
    ],
);

/// The database system, as the conventions name it (`postgresql`, `redis`).
pub const DB_SYSTEM_NAME: &str = "db.system.name";
/// The database the operation ran in.
pub const DB_NAMESPACE: &str = "db.namespace";
/// What the operation did (`SELECT`, `GET`).
pub const DB_OPERATION_NAME: &str = "db.operation.name";
/// The table or collection it did it to.
pub const DB_COLLECTION_NAME: &str = "db.collection.name";
/// The operation and its target together (`SELECT posts`).
pub const DB_QUERY_SUMMARY: &str = "db.query.summary";
/// The server the operation was sent to.
pub const SERVER_ADDRESS: &str = "server.address";
/// The port it was sent to.
pub const SERVER_PORT: &str = "server.port";

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
    duration: &'static DurationHistogram,
    name: &'a str,
    started: SystemTime,
    elapsed: Duration,
    attributes: &'a [(&'static str, ClientValue<'a>)],
    error_type: Option<&'a str>,
    announced: Option<&'a Correlation>,
}

impl<'a> ClientOperation<'a> {
    /// The operation `name`, recorded in `duration`, started at `started` and
    /// answered `elapsed` later, carrying `attributes`.
    pub const fn new(
        duration: &'static DurationHistogram,
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
            announced: None,
        }
    }

    /// The same operation, failed with the class `failure` names — a status
    /// code its protocol answered, or `_OTHER` — if it failed.
    pub const fn failed(self, failure: Option<&'a str>) -> Self {
        Self {
            error_type: failure,
            ..self
        }
    }

    /// The same operation, announced to its peer as `announced` — the
    /// `traceparent` it carried naming its span as the parent — so its span
    /// takes that span's id.
    pub const fn announced_as(self, announced: Option<&'a Correlation>) -> Self {
        Self { announced, ..self }
    }

    /// The histogram it records in.
    pub const fn duration(&self) -> &'static DurationHistogram {
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

    /// What it was announced to its peer as, if it was.
    pub const fn announced(&self) -> Option<&'a Correlation> {
        self.announced
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

    #[test]
    fn an_operation_is_failed_by_its_class_and_keeps_the_rest() {
        let started = SystemTime::now();
        let attributes = [(DB_SYSTEM_NAME, ClientValue::Text("postgresql"))];
        let operation = ClientOperation::new(
            &DB_CLIENT_OPERATION_DURATION,
            "SELECT posts",
            started,
            Duration::from_millis(3),
            &attributes,
        );
        assert_eq!(operation.error_type(), None);

        assert_eq!(operation.failed(None).error_type(), None);
        let failed = operation.failed(Some("_OTHER"));
        assert_eq!(failed.error_type(), Some("_OTHER"));
        assert_eq!(failed.name(), "SELECT posts");
        assert_eq!(failed.started(), started);
        assert_eq!(failed.elapsed(), Duration::from_millis(3));
        assert_eq!(failed.attributes(), &attributes);
        assert_eq!(failed.duration(), &DB_CLIENT_OPERATION_DURATION);
        assert!(failed.announced().is_none());

        let announced = Correlation::minted(None).child();
        assert_eq!(
            failed
                .announced_as(Some(&announced))
                .announced()
                .map(Correlation::span_id),
            Some(announced.span_id()),
        );
    }
}

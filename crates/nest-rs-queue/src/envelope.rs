//! The wire envelope every job travels in, sealed and opened by this crate
//! alone; the format is specified on [`Envelope`].

use std::borrow::Cow;

use nest_rs_core::{Correlation, TraceParent, TraceState};
use serde_json::{Map, Value, json};

use crate::push_options::check_unique_key;
use crate::{JobError, JobId};

/// Wire-format version every job is sealed with. Bumping it makes a worker of
/// the previous release refuse the new envelope — failing the job closed rather
/// than misreading it — during a rolling deploy.
pub const WIRE_FORMAT_VERSION: u32 = 1;

/// The envelope's version key.
const VERSION: &str = "v";
/// Where the developer's payload lives.
const PAYLOAD: &str = "payload";
/// The producer's W3C trace context — its span becomes the job's parent.
const TRACEPARENT: &str = "traceparent";
/// Vendor state, forwarded untouched as W3C Trace Context requires.
const TRACESTATE: &str = "tracestate";
/// Who the enqueue was being served for; absent for a scheduled tick, a boot
/// task or an anonymous caller.
const ACTOR_ID: &str = "actor_id";
/// The [`JobId`] the push minted. Absent from a record no push of this release
/// wrote, which then runs under an id minted for its delivery.
const ID: &str = "id";
/// Which attempt a delivery of this record is, from 1; absent means 1.
const ATTEMPT: &str = "attempt";
/// The unique key the push declared, so whoever settles the job can release it
/// without a lookup of its own.
const UNIQUE_KEY: &str = "unique_key";
/// `true` when `traceparent` is the trace a consumer minted at the job's first
/// attempt; absent means it is the producer's.
const TRACE_MINTED: &str = "trace_minted";
/// Every key an envelope may carry: an object with any other is no envelope.
const KEYS: [&str; 9] = [
    VERSION,
    ID,
    ATTEMPT,
    PAYLOAD,
    TRACEPARENT,
    TRACESTATE,
    ACTOR_ID,
    UNIQUE_KEY,
    TRACE_MINTED,
];

/// A sealed job: the developer's payload in the wire envelope, stamped with the
/// ambient trace context.
///
/// Made only by a push, so a backend cannot store a job that skipped the seal;
/// it stores [`into_json`](Self::into_json) as it is, and hands that value back
/// to the port when it delivers the job.
///
/// ```json
/// {
///   "v": 1,
///   "id": "01890a5d-ac96-774b-bcce-b302099a8057",
///   "attempt": 1,
///   "payload": { "…the developer's payload…": true },
///   "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
///   "tracestate": "rojo=00f067aa0ba902b7",
///   "actor_id": "…",
///   "unique_key": "…"
/// }
/// ```
///
/// `id` is the job's [`JobId`], minted by the push, and `attempt` the attempt a
/// delivery of this record runs, from 1 — the port files a job for a later
/// attempt with the record [`Disposition::Retry`](crate::Disposition::Retry)
/// carries, which holds the next number. `unique_key` is present when the push
/// declared one. A record without `id` or `attempt` is still an envelope: it
/// runs under an id minted for its delivery, as attempt 1.
///
/// `trace_minted` is written only on a record re-filed for a later attempt, and
/// only as `true`: its `traceparent` is then the trace the first attempt minted,
/// which the later attempts join with `continued_trace=false`.
///
/// `traceparent` and `tracestate` are W3C Trace Context's, verbatim: the
/// producer's span becomes the job's parent. `actor_id` is nestrs' own key.
///
/// # What a later version keeps
///
/// A consumer meeting a version newer than its own reads three keys of it and
/// nothing else: `id`, and `traceparent` with the `tracestate` beside it, when
/// they are spelled as this version spells them. It hands the job back unread
/// under that id, files its lines in that trace, and counts the job's wait
/// unread against that id — dead-lettering it a day after its first hand-back
/// ([`NEWER_RELEASE_PATIENCE`](crate::NEWER_RELEASE_PATIENCE)), and at
/// once when it names no id it can read. So a later version keeps those three
/// keys and their spelling, whatever else it changes.
///
/// # A value that is no envelope is normal, not an error
///
/// A value that is not an envelope — not an object carrying a numeric `v` and a
/// `payload`, and nothing but the keys above — is decoded as the payload itself,
/// with a `warn`, and starts a trace of its own.
#[derive(Clone, PartialEq)]
pub struct Envelope {
    json: Value,
    id: JobId,
}

/// The job's id, never the payload (`CLAUDE.md`, no payload value in a line).
impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Envelope {
    /// The id of the job the envelope carries — what a backend keys the job's
    /// own records by.
    pub fn id(&self) -> &JobId {
        &self.id
    }

    /// The unique key the push declared, if any.
    pub fn unique_key(&self) -> Option<&str> {
        self.json.get(UNIQUE_KEY).and_then(Value::as_str)
    }

    /// The envelope as the JSON a backend stores, by value.
    pub fn into_json(self) -> Value {
        self.json
    }
}

/// Wrap a serialized payload in the wire envelope, stamped with the ambient
/// correlation.
///
/// With no ambient context — a job enqueued from `main` — one is minted.
pub(crate) fn seal(payload: Value, id: JobId, unique_key: Option<&str>) -> Envelope {
    let correlation =
        nest_rs_core::__private::current_correlation().unwrap_or_else(|| Correlation::minted(None));
    let mut sealed = json!({
        VERSION: WIRE_FORMAT_VERSION,
        ID: id.to_string(),
        ATTEMPT: 1,
        PAYLOAD: payload,
        TRACEPARENT: correlation.traceparent().to_string(),
    });
    if let Some(unique_key) = unique_key {
        sealed[UNIQUE_KEY] = Value::String(unique_key.to_owned());
    }
    if let Some(tracestate) = correlation.tracestate().as_str() {
        sealed[TRACESTATE] = Value::String(tracestate.to_owned());
    }
    // A worker holds no credential: the actor travels here or is lost.
    if let Some(actor_id) = correlation.actor_id() {
        sealed[ACTOR_ID] = Value::String(actor_id.to_owned());
    }
    Envelope { json: sealed, id }
}

/// What a stored record says about the job it carries, read without opening
/// it: the id, the attempt and the unique key a current envelope names, each
/// `None` when the record names none — or names one this consumer cannot use,
/// which [`open`] flags.
#[derive(Debug, Default)]
pub(crate) struct Identity {
    pub(crate) id: Option<JobId>,
    pub(crate) attempt: Option<u32>,
    pub(crate) unique_key: Option<String>,
}

/// Read what `value` says about its job. A current envelope says everything; a
/// newer release's its id alone, when spelled as this release does, since its
/// other keys may mean what this release does not; anything else nothing.
pub(crate) fn identify(value: &Value) -> Identity {
    let Value::Object(map) = value else {
        return Identity::default();
    };
    let current = u64::from(WIRE_FORMAT_VERSION);
    match envelope_version(map) {
        Some(version) if version == current => Identity {
            id: map.get(ID).and_then(usable_id),
            attempt: map.get(ATTEMPT).and_then(usable_attempt),
            unique_key: map.get(UNIQUE_KEY).and_then(usable_unique_key),
        },
        Some(version) if version > current => Identity {
            id: map.get(ID).and_then(usable_id),
            ..Identity::default()
        },
        _ => Identity::default(),
    }
}

/// The version of `value` when it is an envelope from a newer release than this
/// one — a job this consumer cannot read, and must hand back rather than end.
pub(crate) fn newer_version(value: &Value) -> Option<u64> {
    let Value::Object(map) = value else {
        return None;
    };
    envelope_version(map).filter(|version| *version > u64::from(WIRE_FORMAT_VERSION))
}

/// The trace a newer release's envelope carries, when it spells `traceparent`
/// — and the `tracestate` beside it — as this release does. The actor is never
/// read: an audit identity is not one to guess.
pub(crate) fn newer_trace(value: &Value) -> Option<Correlation> {
    newer_version(value)?;
    let map = value.as_object()?;
    let parent = map
        .get(TRACEPARENT)
        .and_then(Value::as_str)
        .and_then(TraceParent::parse)?;
    let state = map
        .get(TRACESTATE)
        .and_then(Value::as_str)
        .map(TraceState::adopt)
        .unwrap_or_default();
    Some(Correlation::continued(parent, state, None))
}

fn usable_id(value: &Value) -> Option<JobId> {
    value.as_str().and_then(|raw| JobId::parse(raw).ok())
}

fn usable_attempt(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|attempt| *attempt >= 1)
        .and_then(|attempt| u32::try_from(attempt).ok())
}

/// A key a push could have declared — the same rule the push checks.
fn usable_unique_key(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|key| check_unique_key(key).is_ok())
        .map(str::to_owned)
}

/// An actor that names somebody: an empty one names nobody, and the kernel
/// refuses to record one.
fn usable_actor(value: &Value) -> Option<&str> {
    value.as_str().filter(|actor| !actor.is_empty())
}

/// Whether a `tracestate` beside a usable `traceparent` is lost: a list naming a
/// member that cannot be adopted. A list naming no member is a valid one saying
/// nothing — W3C Trace Context has vendors accept an empty `tracestate`.
fn unusable_tracestate(value: &Value) -> bool {
    let names_a_member = value
        .as_str()
        .is_none_or(|list| list.contains(|c: char| !matches!(c, ' ' | '\t' | ',')));
    names_a_member
        && value
            .as_str()
            .map(TraceState::adopt)
            .unwrap_or_default()
            .as_str()
            .is_none()
}

/// The record a backend re-files for attempt number `attempt` of the job
/// delivered as `message`: the same envelope, stamped with the job's `id` and
/// the new attempt number; every other key the push wrote travels unchanged.
///
/// A record whose trace the delivery could not continue carries `minted`, marked
/// as minted. A value that was no envelope is sealed whole as the payload of one.
/// Once the delivery has `announced` the keys it could not use, they are left
/// out. A newer release's envelope travels exactly as it came.
pub(crate) fn retry(
    message: &Value,
    id: &JobId,
    attempt: u32,
    minted: Option<&Correlation>,
    announced: bool,
) -> Envelope {
    let current = u64::from(WIRE_FORMAT_VERSION);
    let mut map = match message {
        Value::Object(map) => match envelope_version(map) {
            Some(version) if version == current => map.clone(),
            Some(version) if version > current => {
                return Envelope {
                    json: message.clone(),
                    id: id.clone(),
                };
            }
            _ => sealed_whole(message),
        },
        other => sealed_whole(other),
    };
    map.insert(VERSION.to_owned(), json!(WIRE_FORMAT_VERSION));
    map.insert(ID.to_owned(), Value::String(id.to_string()));
    map.insert(ATTEMPT.to_owned(), json!(attempt));
    let continued = map
        .get(TRACEPARENT)
        .and_then(Value::as_str)
        .and_then(TraceParent::parse)
        .is_some();
    if announced {
        // What the delivery said it ran without is not filed again.
        if map
            .get(ACTOR_ID)
            .is_some_and(|actor| usable_actor(actor).is_none())
        {
            map.remove(ACTOR_ID);
        }
        if map
            .get(UNIQUE_KEY)
            .is_some_and(|key| usable_unique_key(key).is_none())
        {
            map.remove(UNIQUE_KEY);
        }
        if continued && map.get(TRACESTATE).is_some_and(unusable_tracestate) {
            map.remove(TRACESTATE);
        }
    }
    if !continued && let Some(minted) = minted {
        map.insert(
            TRACEPARENT.to_owned(),
            Value::String(minted.traceparent().to_string()),
        );
        map.insert(TRACE_MINTED.to_owned(), Value::Bool(true));
        // Vendor state belongs to the trace it rode with, and there was none.
        map.remove(TRACESTATE);
        match minted.actor_id() {
            Some(actor_id) => {
                map.insert(ACTOR_ID.to_owned(), Value::String(actor_id.to_owned()));
            }
            // The first attempt ran for nobody: an actor key left here named
            // nobody usable, and was said.
            None => {
                map.remove(ACTOR_ID);
            }
        }
    }
    Envelope {
        json: Value::Object(map),
        id: id.clone(),
    }
}

/// A value that was no envelope, sealed whole as the payload of one.
fn sealed_whole(value: &Value) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert(PAYLOAD.to_owned(), value.clone());
    map
}

/// What a stored value turned out to be.
pub(crate) enum Opened<'a> {
    /// A current envelope: the developer's payload, and what to run it under —
    /// the trace it carried, continued; a trace minted for the actor it named
    /// when the trace context beside it was missing or unusable; `None` when it
    /// carried neither.
    Sealed {
        payload: Cow<'a, Value>,
        correlation: Option<Correlation>,
        /// The trace carried is the one a consumer minted at the job's first
        /// attempt, not the producer's.
        minted: bool,
        /// The keys the envelope carried and this consumer could not use.
        unusable: Unusable,
    },
    /// A value that is no envelope at all, decoded as the payload itself.
    Unversioned(Cow<'a, Value>),
}

/// The shape, never the payload.
impl std::fmt::Debug for Opened<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sealed {
                minted, unusable, ..
            } => f
                .debug_struct("Sealed")
                .field("minted", minted)
                .field("unusable", unusable)
                .finish_non_exhaustive(),
            Self::Unversioned(_) => f.write_str("Unversioned"),
        }
    }
}

/// Which of an envelope's keys were present and unusable.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unusable {
    pub(crate) id: bool,
    pub(crate) attempt: bool,
    pub(crate) traceparent: bool,
    pub(crate) tracestate: bool,
    pub(crate) actor_id: bool,
    pub(crate) unique_key: bool,
}

impl Unusable {
    pub(crate) fn any(self) -> bool {
        self.id
            || self.attempt
            || self.traceparent
            || self.tracestate
            || self.actor_id
            || self.unique_key
    }

    /// The unusable keys, as the envelope names them.
    pub(crate) fn keys(self) -> String {
        [
            (self.id, ID),
            (self.attempt, ATTEMPT),
            (self.traceparent, TRACEPARENT),
            (self.tracestate, TRACESTATE),
            (self.actor_id, ACTOR_ID),
            (self.unique_key, UNIQUE_KEY),
        ]
        .into_iter()
        .filter_map(|(unusable, key)| unusable.then_some(key))
        .collect::<Vec<_>>()
        .join(", ")
    }

    /// What the job runs without, said for the keys that were unusable.
    pub(crate) fn hint(self) -> String {
        let mut effects = Vec::new();
        if self.id {
            effects.push("runs under an id minted for this delivery");
        }
        if self.attempt {
            effects.push("counts its attempts from 1");
        }
        if self.traceparent {
            effects.push("runs under a trace of its own");
        } else if self.tracestate {
            effects.push("continues its trace without the vendor state beside it");
        }
        if self.actor_id {
            effects.push("runs for no actor");
        }
        if self.unique_key {
            effects.push("names no unique key to release when it ends");
        }
        format!(
            "the push never writes these; the record was written around the producer or damaged, \
             and the job {}",
            effects.join(", and ")
        )
    }
}

/// Open a stored value: a current envelope's payload and trace context, a value
/// that is no envelope, or a non-retryable refusal for an envelope of another
/// version — worded for the direction the release gap runs.
///
/// The producer's trace is trusted here, and only here: our own push wrote the
/// envelope into infrastructure the deployment owns.
pub(crate) fn open<'a>(value: Cow<'a, Value>, queue: &str) -> Result<Opened<'a>, JobError> {
    let Value::Object(map) = value.as_ref() else {
        return Ok(Opened::Unversioned(value));
    };
    let Some(version) = envelope_version(map) else {
        return Ok(Opened::Unversioned(value));
    };
    let current = u64::from(WIRE_FORMAT_VERSION);
    if version > current {
        return Err(JobError::abort(format!(
            "unsupported job wire-format version {version} on queue `{queue}`; the producer is \
             from a newer release, which this consumer cannot read",
        )));
    }
    if version < current {
        // The first version this crate ever wrote is 1: anything below it was
        // never a release's, so no consumer can be pinned at it.
        let remedy = if version == 0 {
            "no release wrote it; the record is a foreign producer's, and only \
             draining the queue clears it"
        } else {
            "the producer is from an older release; either drain the queue or pin the \
             consumer at that version"
        };
        return Err(JobError::abort(format!(
            "unsupported job wire-format version {version} on queue `{queue}`; {remedy}",
        )));
    }
    let traceparent = map.get(TRACEPARENT);
    let parent = traceparent
        .and_then(Value::as_str)
        .and_then(TraceParent::parse);
    let tracestate = map.get(TRACESTATE);
    let state = tracestate
        .and_then(Value::as_str)
        .map(TraceState::adopt)
        .unwrap_or_default();
    let minted = map.get(TRACE_MINTED) == Some(&Value::Bool(true)) && parent.is_some();
    let actor = map.get(ACTOR_ID);
    let actor_id = actor.and_then(usable_actor);
    let unusable = Unusable {
        id: map.get(ID).is_some_and(|id| usable_id(id).is_none()),
        attempt: map
            .get(ATTEMPT)
            .is_some_and(|attempt| usable_attempt(attempt).is_none()),
        traceparent: traceparent.is_some() && parent.is_none(),
        // A vendor state means something only beside the trace it rides with.
        tracestate: parent.is_some() && tracestate.is_some_and(unusable_tracestate),
        actor_id: actor.is_some() && actor_id.is_none(),
        unique_key: map
            .get(UNIQUE_KEY)
            .is_some_and(|key| usable_unique_key(key).is_none()),
    };
    // The actor is inherited, never re-derived, and survives a missing or
    // corrupt `traceparent`: the audit identity is not the trace's to lose.
    let correlation = match parent {
        Some(parent) => Some(Correlation::continued(parent, state, actor_id)),
        None => actor_id.map(|actor| Correlation::minted(Some(actor))),
    };
    let payload = match value {
        Cow::Borrowed(value) => value
            .get(PAYLOAD)
            .map_or(Cow::Owned(Value::Null), Cow::Borrowed),
        Cow::Owned(mut value) => Cow::Owned(
            value
                .get_mut(PAYLOAD)
                .map(Value::take)
                .unwrap_or(Value::Null),
        ),
    };
    Ok(Opened::Sealed {
        payload,
        correlation,
        minted,
        unusable,
    })
}

/// The version of an envelope, or `None` when the object is not one.
///
/// Strict, so a developer's payload that happens to carry a `v` and a
/// `payload` is not taken for an envelope: `v` must be a non-negative whole
/// number — `1` or a hand-rolled producer's `1.0` — `payload` must be present,
/// and nothing may sit beside them but the envelope's own keys — unless `v` is
/// newer than this release's, whose envelope may add keys of its own.
///
/// The identity keys must hold the types the push writes: `{"v": 2, "payload": …,
/// "id": 42}` is a developer's own record, not a newer release's envelope.
fn envelope_version(map: &Map<String, Value>) -> Option<u64> {
    /// 2^64, the first whole number a `u64` cannot hold — exactly representable
    /// as an `f64`.
    const TWO_TO_THE_64: f64 = 18_446_744_073_709_551_616.0;

    if !map.contains_key(PAYLOAD) {
        return None;
    }
    let ours = map.get(ID).is_none_or(Value::is_string)
        && map.get(ATTEMPT).is_none_or(Value::is_number)
        && map.get(UNIQUE_KEY).is_none_or(Value::is_string)
        && map.get(TRACE_MINTED).is_none_or(Value::is_boolean);
    if !ours {
        return None;
    }
    let Value::Number(number) = map.get(VERSION)? else {
        return None;
    };
    let version = number.as_u64().or_else(|| {
        number
            .as_f64()
            .filter(|float| {
                float.is_finite()
                    && *float >= 0.0
                    && float.fract() == 0.0
                    // Saturating would read `1e300` as a newer release; and
                    // `u64::MAX as f64` rounds *up* to 2^64, hence `<`.
                    && *float < TWO_TO_THE_64
            })
            .map(|float| float as u64)
    })?;
    let only_ours = map.keys().all(|key| KEYS.contains(&key.as_str()));
    (only_ours || version > u64::from(crate::WIRE_FORMAT_VERSION)).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(value: Value) -> Opened<'static> {
        open(Cow::Owned(value), "audio").expect("opens")
    }

    fn under<F: std::future::Future>(
        correlation: Correlation,
        fut: F,
    ) -> impl std::future::Future<Output = F::Output> {
        nest_rs_core::with_request_scope(None, correlation, fut)
    }

    #[test]
    fn no_debug_rendering_quotes_the_payload() {
        let secret = json!({ "card": "4242-SECRET" });
        let sealed = seal(secret.clone(), JobId::mint(), None);
        assert!(!format!("{sealed:?}").contains("SECRET"), "{sealed:?}");
        let opened = opened(sealed.into_json());
        assert!(!format!("{opened:?}").contains("SECRET"), "{opened:?}");
        let foreign = Opened::Unversioned(Cow::Owned(secret));
        assert!(!format!("{foreign:?}").contains("SECRET"), "{foreign:?}");
    }

    #[test]
    fn a_sealed_job_opens_to_exactly_its_payload() {
        let payload = json!({ "file": "song.wav" });
        let Opened::Sealed {
            payload: opened,
            correlation,
            ..
        } = opened(seal(payload.clone(), JobId::mint(), None).into_json())
        else {
            panic!("a sealed job opens as sealed");
        };
        assert_eq!(*opened, payload);
        assert!(correlation.is_some(), "and the context travelled with it");
    }

    #[test]
    fn a_borrowed_record_opens_to_its_payload_in_place() {
        let stored = seal(json!({ "file": "song.wav" }), JobId::mint(), None).into_json();
        let Ok(Opened::Sealed {
            payload: Cow::Borrowed(read),
            ..
        }) = open(Cow::Borrowed(&stored), "audio")
        else {
            panic!("a borrowed record opens to a borrowed payload");
        };
        assert!(std::ptr::eq(read, &stored[PAYLOAD]));
        let Ok(Opened::Sealed {
            payload: Cow::Owned(taken),
            ..
        }) = open(Cow::Owned(stored.clone()), "audio")
        else {
            panic!("an owned record opens to an owned payload");
        };
        assert_eq!(taken, stored[PAYLOAD]);
    }

    #[tokio::test]
    async fn the_job_is_a_child_of_the_enqueue_in_the_same_trace() {
        let producer = Correlation::minted(None);
        let sealed = under(producer.clone(), async {
            seal(json!({ "clip": 1 }), JobId::mint(), None)
        })
        .await;

        let Opened::Sealed { correlation, .. } = opened(sealed.into_json()) else {
            panic!("sealed");
        };
        let adopted = correlation.expect("the context travelled");
        assert_eq!(adopted.trace_id(), producer.trace_id(), "one trace");
        assert_eq!(
            adopted.parent_id(),
            Some(producer.span_id()),
            "the job names the enqueue that caused it",
        );
        assert_ne!(
            adopted.span_id(),
            producer.span_id(),
            "and is its own unit of work"
        );
    }

    #[tokio::test]
    async fn tracestate_crosses_the_process_boundary_verbatim() {
        let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .expect("the spec's own example");
        let producer = Correlation::continued(
            parent,
            TraceState::adopt("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE"),
            None,
        );

        let sealed = under(producer, async {
            seal(json!({ "clip": 1 }), JobId::mint(), None)
        })
        .await;
        let Opened::Sealed { correlation, .. } = opened(sealed.into_json()) else {
            panic!("sealed");
        };
        assert_eq!(
            correlation
                .expect("context travelled")
                .tracestate()
                .as_str(),
            Some("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE"),
        );
    }

    #[tokio::test]
    async fn the_actor_crosses_with_the_trace() {
        let sealed = under(Correlation::minted(None), async {
            nest_rs_core::__private::set_actor_id("alice-42");
            seal(json!({ "clip": 1 }), JobId::mint(), None)
        })
        .await;

        let Opened::Sealed { correlation, .. } = opened(sealed.into_json()) else {
            panic!("sealed");
        };
        assert_eq!(
            correlation.as_ref().and_then(Correlation::actor_id),
            Some("alice-42")
        );
    }

    #[tokio::test]
    async fn an_anonymous_enqueue_carries_a_trace_and_no_actor() {
        let sealed = under(Correlation::minted(None), async {
            seal(json!({ "clip": 1 }), JobId::mint(), None)
        })
        .await;
        let Opened::Sealed { correlation, .. } = opened(sealed.into_json()) else {
            panic!("sealed");
        };
        assert!(correlation.is_some(), "the job is still correlated");
        assert_eq!(correlation.as_ref().and_then(Correlation::actor_id), None);
    }

    #[test]
    fn a_value_that_is_no_envelope_is_the_payload_itself() {
        for bare in [
            json!({ "clip": 1 }),
            json!("just a string"),
            json!(42),
            json!(null),
            json!({ "v": 9, "payload": 100, "id": 42 }),
            json!({ "v": 1, "payload": { "n": 1 }, "id": 42 }),
            json!({ "v": 1, "payload": { "n": 1 }, "attempt": "3" }),
            json!({ "v": "1", "payload": { "n": 1 } }),
            json!({ "v": -1, "payload": { "n": 1 } }),
            json!({ "v": 18_446_744_073_709_551_616.0_f64, "payload": { "n": 1 } }),
            json!({ "v": 1e300, "payload": { "n": 1 } }),
            json!({ "v": 1, "payload": { "n": 1 }, "trace_minted": "yes" }),
            json!({ "v": 1 }),
        ] {
            let Opened::Unversioned(value) = opened(bare.clone()) else {
                panic!("{bare} is not an envelope");
            };
            assert_eq!(*value, bare, "nothing is reshaped");
        }
    }

    #[test]
    fn a_float_version_and_a_missing_or_unusable_context_still_open_the_payload() {
        for envelope in [
            json!({ "v": 1.0, "payload": { "clip": 1 } }),
            json!({ "v": 1, "payload": { "clip": 1 } }),
            json!({ "v": 1, "payload": { "clip": 1 }, "traceparent": "not-a-traceparent" }),
        ] {
            let Opened::Sealed {
                payload,
                correlation,
                ..
            } = opened(envelope.clone())
            else {
                panic!("{envelope} is an envelope");
            };
            assert_eq!(*payload, json!({ "clip": 1 }));
            assert!(correlation.is_none(), "{envelope}");
        }
    }

    #[test]
    fn another_version_is_refused_naming_the_direction_of_the_release_gap() {
        let newer = open(
            Cow::Owned(json!({ "v": u64::from(WIRE_FORMAT_VERSION) + 99, "payload": {} })),
            "audio",
        )
        .expect_err("a newer version is refused");
        assert!(!newer.retryable, "a version never changes on retry");
        let newer = newer.to_string();
        assert!(
            newer.contains("newer release") && newer.contains("cannot read"),
            "{newer}"
        );
        assert!(newer.contains("`audio`"), "{newer}");

        let foreign = open(Cow::Owned(json!({ "v": 0, "payload": {} })), "audio")
            .expect_err("a version below the first is refused")
            .to_string();
        assert!(
            foreign.contains("no release wrote it") && foreign.contains("draining the queue"),
            "{foreign}"
        );
        assert!(!foreign.contains("pin the consumer"), "{foreign}");
    }

    #[test]
    fn unusable_keys_are_flagged_where_the_job_runs_without_them() {
        let traceparent = Correlation::minted(None).traceparent().to_string();
        let flagged = |envelope: Value| {
            let Opened::Sealed { unusable, .. } = opened(envelope.clone()) else {
                panic!("{envelope} is an envelope");
            };
            unusable.keys()
        };
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "actor_id": "" })),
            "actor_id"
        );
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "traceparent": traceparent, "tracestate": 7 })),
            "tracestate"
        );
        assert_eq!(
            flagged(
                json!({ "v": 1, "payload": {}, "traceparent": traceparent, "tracestate": "rojo=\u{1}" })
            ),
            "tracestate",
            "a list naming a member nothing can adopt"
        );
        for empty in ["", "   ", ",,", " , \t"] {
            assert_eq!(
                flagged(
                    json!({ "v": 1, "payload": {}, "traceparent": traceparent, "tracestate": empty })
                ),
                "",
                "an empty list names no vendor state: {empty:?}"
            );
        }
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "tracestate": 7 })),
            ""
        );
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "traceparent": 7, "actor_id": null })),
            "traceparent, actor_id"
        );
    }

    #[test]
    fn a_sealed_job_carries_its_id_its_first_attempt_and_its_unique_key() {
        let id = JobId::mint();
        let sealed = seal(json!({ "clip": 1 }), id.clone(), Some("clip-1"));
        assert_eq!(sealed.id(), &id);
        assert_eq!(sealed.unique_key(), Some("clip-1"));

        let json = sealed.into_json();
        assert_eq!(json[ID], json!(id.to_string()));
        assert_eq!(json[ATTEMPT], json!(1));
        let identity = identify(&json);
        assert_eq!(identity.id, Some(id));
        assert_eq!(identity.attempt, Some(1));
        assert_eq!(identity.unique_key.as_deref(), Some("clip-1"));

        let plain = seal(json!({}), JobId::mint(), None).into_json();
        assert!(
            plain.get(UNIQUE_KEY).is_none(),
            "no key declared, none written"
        );
    }

    #[test]
    fn only_a_current_envelope_names_its_job_and_a_newer_one_its_id_alone() {
        let id = JobId::mint().to_string();
        for silent in [
            json!({ "id": id, "attempt": 3 }),
            json!({ "v": 0, "id": id, "attempt": 3, "payload": {} }),
            json!("a string"),
        ] {
            let identity = identify(&silent);
            assert!(
                identity.id.is_none() && identity.attempt.is_none(),
                "{silent}"
            );
        }
        let legacy = identify(&json!({ "v": 1, "payload": {} }));
        assert!(legacy.id.is_none() && legacy.attempt.is_none());

        let newer =
            identify(&json!({ "v": 2, "id": id, "attempt": 3, "unique_key": "k", "payload": {} }));
        assert_eq!(newer.id.map(|id| id.to_string()), Some(id));
        assert!(newer.attempt.is_none() && newer.unique_key.is_none());
        assert_eq!(
            newer_version(&json!({ "v": 2, "payload": {} })),
            Some(2),
            "and it is known as newer"
        );
        assert_eq!(newer_version(&json!({ "v": 1, "payload": {} })), None);
    }

    #[test]
    fn a_newer_envelope_lends_its_trace_and_nothing_else() {
        let parent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let newer = json!({
            "v": 2,
            "traceparent": parent,
            "tracestate": "rojo=00f067aa0ba902b7",
            "actor_id": "user-7",
            "payload": {},
        });
        let trace = newer_trace(&newer).expect("the newer envelope's trace");
        assert_eq!(
            trace.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(
            trace.parent_id().map(|span| span.to_string()).as_deref(),
            Some("00f067aa0ba902b7")
        );
        assert!(trace.parent_is_remote());
        assert_eq!(trace.tracestate().as_str(), Some("rojo=00f067aa0ba902b7"));
        assert_eq!(
            trace.actor_id(),
            None,
            "an audit identity is not guessed at"
        );

        for none in [
            json!({ "v": 1, "traceparent": parent, "payload": {} }),
            json!({ "v": 2, "traceparent": "not a traceparent", "payload": {} }),
            json!({ "v": 2, "payload": {} }),
        ] {
            assert!(newer_trace(&none).is_none(), "{none}");
        }
    }

    #[test]
    fn an_unusable_identity_key_is_flagged() {
        let flagged = |envelope: Value| {
            let Opened::Sealed { unusable, .. } = opened(envelope.clone()) else {
                panic!("{envelope} is an envelope");
            };
            unusable.keys()
        };
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "id": "job-0" })),
            "id"
        );
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "attempt": 0 })),
            "attempt"
        );
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "unique_key": "a\nb" })),
            "unique_key"
        );
        let id = JobId::mint().to_string();
        assert_eq!(
            flagged(json!({ "v": 1, "payload": {}, "id": id, "attempt": 4 })),
            ""
        );
    }

    #[tokio::test]
    async fn a_retried_record_is_the_same_envelope_one_attempt_on() {
        let producer = Correlation::minted(Some("alice"));
        let id = JobId::mint();
        let sealed = under(producer.clone(), async {
            seal(json!({ "clip": 1 }), id.clone(), Some("clip-1"))
        })
        .await
        .into_json();

        let retried = retry(&sealed, &id, 2, None, true);
        assert_eq!(retried.id(), &id);
        let json = retried.into_json();
        assert_eq!(json[ATTEMPT], json!(2));
        for key in [ID, VERSION, PAYLOAD, TRACEPARENT, ACTOR_ID, UNIQUE_KEY] {
            assert_eq!(json[key], sealed[key], "`{key}` travels unchanged");
        }
        assert_eq!(identify(&json).attempt, Some(2));
    }

    #[test]
    fn a_retried_record_without_a_trace_joins_the_first_attempts() {
        let first = Correlation::minted(Some("bob"));
        let id = JobId::mint();
        for stored in [
            json!({ "clip": 1 }),
            json!({ "v": 1, "payload": { "clip": 1 } }),
        ] {
            let json = retry(&stored, &id, 2, Some(&first), true).into_json();
            assert_eq!(
                json[TRACEPARENT],
                json!(first.traceparent().to_string()),
                "{stored}"
            );
            assert_eq!(json[ACTOR_ID], json!("bob"));
            let Opened::Sealed { payload, .. } = opened(json.clone()) else {
                panic!("{json} is an envelope");
            };
            assert_eq!(*payload, json!({ "clip": 1 }), "the payload is kept whole");
        }
    }

    #[test]
    fn an_actor_survives_a_missing_or_unusable_trace_context() {
        for envelope in [
            json!({ "v": 1, "payload": {}, "actor_id": "bob" }),
            json!({ "v": 1, "payload": {}, "actor_id": "bob", "traceparent": "corrupt" }),
        ] {
            let Opened::Sealed { correlation, .. } = opened(envelope.clone()) else {
                panic!("{envelope} is an envelope");
            };
            let correlation = correlation.expect("a trace minted for the actor");
            assert_eq!(correlation.actor_id(), Some("bob"), "{envelope}");
            assert!(
                !correlation.parent_is_remote(),
                "minted, not continued: nothing was there to continue"
            );
        }
    }
}

//! The wire envelope every job travels in, sealed and opened by this crate
//! alone.
//!
//! The module is private and [`Envelope`] is re-exported flat, so **the format
//! is specified on the type** rather than here: a `//!` on a private module
//! renders nowhere, and this one is the interop contract a third-party driver
//! reads.

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
/// Vendor state, forwarded untouched because the specification requires every
/// participant to forward it, including the ones that understand none of it.
const TRACESTATE: &str = "tracestate";
/// Who the enqueue was being served for, when anyone was. Absent for a job
/// enqueued by a scheduled tick, a boot task, or an anonymous caller — and
/// absent is the answer, not a missing one.
const ACTOR_ID: &str = "actor_id";
/// The [`JobId`] the push minted. Absent from a record no push of this release
/// wrote, which then runs under an id minted for its delivery.
const ID: &str = "id";
/// Which attempt a delivery of this record is, from 1. Absent means 1: a
/// record re-filed for a later attempt carries it, the push's own does too.
const ATTEMPT: &str = "attempt";
/// The unique key the push declared, so whoever settles the job can release it
/// without a lookup of its own.
const UNIQUE_KEY: &str = "unique_key";

/// A sealed job: the developer's payload in the wire envelope, stamped with the
/// ambient trace context.
///
/// Made only by a push, so a backend cannot store a job that skipped the seal;
/// it stores [`into_json`](Self::into_json) as it is, and hands that value back
/// to the port when it delivers the job.
///
/// The wire envelope every job travels in — and what carries the W3C trace
/// context across the process boundary a queue puts between a producer and its
/// worker.
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
/// delivery of this record runs, from 1 — a backend re-filing a job for a later
/// attempt files the record [`Delivery::retry_envelope`](crate::consume::Delivery::retry_envelope)
/// hands it, which carries the next number. `unique_key` is present when the push
/// declared one. A record without `id` or `attempt` is still an envelope: it
/// runs under an id minted for its delivery, as attempt 1.
///
/// `traceparent` is the standard's own field, verbatim, because this is a
/// *context propagation* boundary and the standard exists for exactly this: the
/// producer's span becomes the job's parent, so a worker running minutes later
/// in another binary appears in the trace of the request that enqueued it —
/// under it, not merely beside it.
///
/// `actor_id` is not W3C's, and that is deliberate rather than an omission. W3C
/// Baggage is the standard for carrying arbitrary key-value context, and it is
/// an **HTTP header** specification; this envelope is nestrs' own format, where
/// an explicit key is the honest spelling. The day the framework propagates
/// context over outbound HTTP, that is Baggage's job and this key does not
/// become one.
///
/// # This crate seals and opens it, and nothing else does
///
/// A push seals the payload before a backend sees it — the backend receives an
/// [`Envelope`], which only this crate can make — and an attempt opens the stored
/// value before the handler runs. A backend moves JSON: it can neither forget the
/// trace context nor misread the version, and the handler never sees a key the
/// framework added.
///
/// # One envelope, not two
///
/// The versioned envelope is what lets a rolling deploy fail closed rather than
/// misinterpret bytes, and the trace context is more keys on it rather than a
/// second wrapper around it: nesting one envelope inside another gives two
/// things to strip, in an order every reader would have to get right.
///
/// # A value that is no envelope is normal, not an error
///
/// A queue is shared infrastructure. A job may predate the envelope, or come from
/// a system that is not this framework at all. A value that is not an envelope —
/// not an object carrying a numeric `v` and a `payload`, and nothing but the keys
/// above — is decoded as the payload itself, with a `warn`, and starts a trace of
/// its own. Refusing it would dead-letter a perfectly good job over an
/// observability field.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    json: Value,
    id: JobId,
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
/// With no ambient context — a job enqueued from `main`, or from a path no edge
/// opened — one is minted: the producer's own events and the consumer's still
/// group together, which is strictly more than nothing.
pub(crate) fn seal(payload: Value, id: JobId, unique_key: Option<&str>) -> Envelope {
    // One read for every key: they are one value on the ambient context for the
    // reason `Correlation`'s own doc gives — an edge that carried the trace while
    // dropping the actor reopens exactly the gap this crossing closes.
    let correlation =
        nest_rs_core::current_correlation().unwrap_or_else(|| Correlation::minted(None));
    let mut sealed = json!({
        VERSION: WIRE_FORMAT_VERSION,
        ID: id.to_string(),
        ATTEMPT: 1,
        PAYLOAD: payload,
        // **This** span is what the job's parent is, which is what makes the job
        // a child of the enqueue rather than a sibling of it.
        TRACEPARENT: correlation.traceparent().to_string(),
    });
    if let Some(unique_key) = unique_key {
        sealed[UNIQUE_KEY] = Value::String(unique_key.to_owned());
    }
    if let Some(tracestate) = correlation.tracestate().as_str() {
        sealed[TRACESTATE] = Value::String(tracestate.to_owned());
    }
    // The actor travels too, and it has to: a worker never re-authenticates —
    // there is no credential on a job — so if the enqueue does not say who it
    // was for, nothing downstream can ever answer it.
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

/// Read what `value` says about its job. Only a current envelope says anything:
/// a value that is no envelope has no keys of ours, and one of another version
/// is refused before its keys could mean what this release means by them.
pub(crate) fn identify(value: &Value) -> Identity {
    let Value::Object(map) = value else {
        return Identity::default();
    };
    if envelope_version(map) != Some(u64::from(WIRE_FORMAT_VERSION)) {
        return Identity::default();
    }
    Identity {
        id: map.get(ID).and_then(usable_id),
        attempt: map.get(ATTEMPT).and_then(usable_attempt),
        unique_key: map.get(UNIQUE_KEY).and_then(usable_unique_key),
    }
}

fn usable_id(value: &Value) -> Option<JobId> {
    value.as_str().and_then(|raw| JobId::parse(raw).ok())
}

/// A whole number of at least one: attempts count from 1.
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

/// The record a backend re-files for attempt number `attempt` of the job
/// delivered as `message`: the same envelope, stamped with the job's `id` and
/// the new attempt number.
///
/// Every key the push wrote travels unchanged — the trace context above all, so
/// each attempt stays a child of the enqueue. A record whose trace the delivery
/// could not continue carries `minted`, the trace its first attempt ran in, so
/// the attempts after it join that trace rather than each starting one. A value
/// that was no envelope is sealed into one, keeping it whole as the payload.
pub(crate) fn retry(
    message: &Value,
    id: &JobId,
    attempt: u32,
    minted: Option<&Correlation>,
) -> Envelope {
    let current = u64::from(WIRE_FORMAT_VERSION);
    let mut map = match message {
        Value::Object(map) if envelope_version(map) == Some(current) => map.clone(),
        other => {
            let mut map = Map::new();
            map.insert(PAYLOAD.to_owned(), other.clone());
            map
        }
    };
    map.insert(VERSION.to_owned(), json!(WIRE_FORMAT_VERSION));
    map.insert(ID.to_owned(), Value::String(id.to_string()));
    map.insert(ATTEMPT.to_owned(), json!(attempt));
    let continued = map
        .get(TRACEPARENT)
        .and_then(Value::as_str)
        .and_then(TraceParent::parse)
        .is_some();
    if !continued && let Some(minted) = minted {
        map.insert(
            TRACEPARENT.to_owned(),
            Value::String(minted.traceparent().to_string()),
        );
        // Vendor state belongs to the trace it rode with, and there was none.
        map.remove(TRACESTATE);
        if let Some(actor_id) = minted.actor_id() {
            map.insert(ACTOR_ID.to_owned(), Value::String(actor_id.to_owned()));
        }
    }
    Envelope {
        json: Value::Object(map),
        id: id.clone(),
    }
}

/// What a stored value turned out to be.
#[derive(Debug)]
pub(crate) enum Opened {
    /// A current envelope: the developer's payload, and what to run it under —
    /// the trace it carried, continued; a trace minted for the actor it named
    /// when the trace context beside it was missing or unusable; `None` when it
    /// carried neither.
    Sealed {
        payload: Value,
        correlation: Option<Correlation>,
        /// The keys the envelope carried and this consumer could not use — a
        /// `traceparent` that does not parse, a `tracestate` beside a usable one
        /// that names a member and cannot be adopted, an `actor_id` that is not a
        /// non-empty string.
        /// Our own push never writes those, so their presence is worth a line:
        /// the record was written around the producer, or damaged.
        unusable: Unusable,
    },
    /// A value that is no envelope at all, decoded as the payload itself.
    Unversioned(Value),
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
/// The producer is trusted here, and only here: an envelope was written by *our
/// own* push into infrastructure the deployment owns, which is what makes
/// continuing its trace sound where an arbitrary HTTP caller's header is not.
pub(crate) fn open(value: Value, queue: &str) -> Result<Opened, JobError> {
    let Value::Object(mut map) = value else {
        return Ok(Opened::Unversioned(value));
    };
    let Some(version) = envelope_version(&map) else {
        return Ok(Opened::Unversioned(Value::Object(map)));
    };
    let current = u64::from(WIRE_FORMAT_VERSION);
    if version > current {
        return Err(JobError::abort(format!(
            "unsupported job wire-format version {version} on queue `{queue}`; the producer is \
             from a newer release; either roll back this consumer or wait for the producer to drain",
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
    let payload = map.remove(PAYLOAD).unwrap_or(Value::Null);
    let traceparent = map.remove(TRACEPARENT);
    let parent = traceparent
        .as_ref()
        .and_then(Value::as_str)
        .and_then(TraceParent::parse);
    let tracestate = map.remove(TRACESTATE);
    let state = tracestate
        .as_ref()
        .and_then(Value::as_str)
        .map(TraceState::adopt)
        .unwrap_or_default();
    // A vendor state means something only beside the trace it rides with, and a
    // list naming no member is a valid one saying nothing: W3C Trace Context has
    // vendors accept an empty `tracestate`.
    let names_a_member = tracestate.as_ref().is_some_and(|value| {
        value
            .as_str()
            .is_none_or(|list| list.contains(|c: char| !matches!(c, ' ' | '\t' | ',')))
    });
    let actor = map.remove(ACTOR_ID);
    // An empty actor names nobody, and the kernel refuses to record one.
    let actor_id = actor
        .as_ref()
        .and_then(Value::as_str)
        .filter(|actor| !actor.is_empty());
    // The job's identity was read by `identify` when the delivery was made; it
    // is looked at here only to say which of its keys the delivery did without.
    let unusable = Unusable {
        id: map.remove(ID).is_some_and(|id| usable_id(&id).is_none()),
        attempt: map
            .remove(ATTEMPT)
            .is_some_and(|attempt| usable_attempt(&attempt).is_none()),
        traceparent: traceparent.is_some() && parent.is_none(),
        tracestate: parent.is_some() && names_a_member && state.as_str().is_none(),
        actor_id: actor.is_some() && actor_id.is_none(),
        unique_key: map
            .remove(UNIQUE_KEY)
            .is_some_and(|key| usable_unique_key(&key).is_none()),
    };
    // The actor is **inherited, never re-derived**: a worker holds no credential
    // and cannot authenticate anyone, so what the enqueue knew is the only
    // answer there will ever be. That holds whether or not the trace context
    // beside it survived: an envelope whose `traceparent` is missing or corrupt
    // starts a trace of its own, and still runs for the actor it names — the
    // audit identity is not the trace's to lose.
    let correlation = match parent {
        Some(parent) => Some(Correlation::continued(parent, state, actor_id)),
        None => actor_id.map(|actor| Correlation::minted(Some(actor))),
    };
    Ok(Opened::Sealed {
        payload,
        correlation,
        unusable,
    })
}

/// The version of an envelope, or `None` when the object is not one.
///
/// Strict, so a developer's payload that happens to carry a `v` and a
/// `payload` is not taken for an envelope: `v` must be a non-negative whole
/// number — `1` or a hand-rolled producer's `1.0` — `payload` must be present,
/// and nothing may sit beside them but the envelope's own keys.
///
/// **The job's identity keys must hold what the push writes** — `id` and
/// `unique_key` a string, `attempt` a number — because `id` is a name a
/// developer's own payload uses as often as any: `{"v": 2, "payload": …, "id":
/// 42}` is someone's record, and reading it as a newer release's envelope would
/// refuse a job that is perfectly good. A string that is no job id, or a number
/// that is no attempt, is still ours, and is flagged rather than guessed at.
fn envelope_version(map: &Map<String, Value>) -> Option<u64> {
    if !map.contains_key(PAYLOAD)
        || !map.keys().all(|key| {
            [
                VERSION,
                ID,
                ATTEMPT,
                PAYLOAD,
                TRACEPARENT,
                TRACESTATE,
                ACTOR_ID,
                UNIQUE_KEY,
            ]
            .contains(&key.as_str())
        })
    {
        return None;
    }
    let ours = map.get(ID).is_none_or(Value::is_string)
        && map.get(ATTEMPT).is_none_or(Value::is_number)
        && map.get(UNIQUE_KEY).is_none_or(Value::is_string);
    if !ours {
        return None;
    }
    let Value::Number(number) = map.get(VERSION)? else {
        return None;
    };
    number.as_u64().or_else(|| {
        number
            .as_f64()
            .filter(|float| {
                float.is_finite()
                    && *float >= 0.0
                    && float.fract() == 0.0
                    // Past what a version can be: a foreign value, not an
                    // envelope — saturating would read `1e300` as a release
                    // newer than every consumer.
                    && *float <= u64::MAX as f64
            })
            .map(|float| float as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(value: Value) -> Opened {
        open(value, "audio").expect("opens")
    }

    fn under<F: std::future::Future>(
        correlation: Correlation,
        fut: F,
    ) -> impl std::future::Future<Output = F::Output> {
        nest_rs_core::with_request_scope(None, correlation, fut)
    }

    /// What the handler decodes is the developer's payload and nothing else —
    /// the trace context rides beside it, it does not reshape it.
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
        assert_eq!(opened, payload);
        assert!(correlation.is_some(), "and the context travelled with it");
    }

    /// The whole point of the boundary crossing: the consumer runs inside the
    /// producer's trace, and **under** its span rather than beside it.
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

    /// Vendor state is a MUST to forward, and its breakage is invisible to us
    /// and fatal to whichever vendor's sampling or routing rides in it.
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

    /// A worker holds no credential, so the actor is only ever what the enqueue
    /// knew. Without it crossing here, every job would be attributed to nobody.
    #[tokio::test]
    async fn the_actor_crosses_with_the_trace() {
        let sealed = under(Correlation::minted(None), async {
            nest_rs_core::set_actor_id("alice-42");
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

    /// An enqueue nobody was authenticated for still correlates; it is simply
    /// attributed to no one, and `None` says exactly that.
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

    /// A queue is shared infrastructure: a value nobody sealed is the payload
    /// itself, untouched — including a developer's own payload that happens to
    /// carry a `v` and a `payload`.
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
            json!({ "v": 1 }),
        ] {
            let Opened::Unversioned(value) = opened(bare.clone()) else {
                panic!("{bare} is not an envelope");
            };
            assert_eq!(value, bare, "nothing is reshaped");
        }
    }

    /// A hand-rolled producer may write `1.0`, and an envelope with no trace
    /// context — or a malformed one — still delivers its payload.
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
            assert_eq!(payload, json!({ "clip": 1 }));
            assert!(correlation.is_none(), "{envelope}");
        }
    }

    /// Another version fails closed and says which way the release gap runs —
    /// a newer producer and an older one are opposite remedies.
    #[test]
    fn another_version_is_refused_naming_the_direction_of_the_release_gap() {
        let newer = open(
            json!({ "v": u64::from(WIRE_FORMAT_VERSION) + 99, "payload": {} }),
            "audio",
        )
        .expect_err("a newer version is refused");
        assert!(!newer.retryable, "a version never changes on retry");
        let newer = newer.to_string();
        assert!(
            newer.contains("newer release") && newer.contains("roll back"),
            "{newer}"
        );
        assert!(newer.contains("`audio`"), "{newer}");

        // Version 1 is the first this crate wrote, so 0 was never a release's:
        // the remedy cannot be to pin a consumer at it.
        let foreign = open(json!({ "v": 0, "payload": {} }), "audio")
            .expect_err("a version below the first is refused")
            .to_string();
        assert!(
            foreign.contains("no release wrote it") && foreign.contains("draining the queue"),
            "{foreign}"
        );
        assert!(!foreign.contains("pin the consumer"), "{foreign}");
    }

    /// A key present and unusable is flagged: an empty actor names nobody, and a
    /// vendor state that cannot be adopted beside a usable trace is dropped. A
    /// vendor state beside no usable trace is not flagged, having nothing to ride,
    /// and neither is an empty list, which is a valid one saying nothing.
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

    /// The push stamps the job's identity beside its payload, and a delivery
    /// reads it back without opening the record.
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

    /// A value that is no envelope, or an envelope of another version, says
    /// nothing about its job: the delivery mints what it needs.
    #[test]
    fn only_a_current_envelope_names_its_job() {
        let id = JobId::mint().to_string();
        for silent in [
            json!({ "id": id, "attempt": 3 }),
            json!({ "v": 2, "id": id, "attempt": 3, "payload": {} }),
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
    }

    /// An identity key present and unusable is flagged, and the delivery runs
    /// without it.
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

    /// The record a later attempt is re-filed as keeps every key the push wrote
    /// and carries the next attempt's number.
    #[tokio::test]
    async fn a_retried_record_is_the_same_envelope_one_attempt_on() {
        let producer = Correlation::minted(Some("alice"));
        let id = JobId::mint();
        let sealed = under(producer.clone(), async {
            seal(json!({ "clip": 1 }), id.clone(), Some("clip-1"))
        })
        .await
        .into_json();

        let retried = retry(&sealed, &id, 2, None);
        assert_eq!(retried.id(), &id);
        let json = retried.into_json();
        assert_eq!(json[ATTEMPT], json!(2));
        for key in [ID, VERSION, PAYLOAD, TRACEPARENT, ACTOR_ID, UNIQUE_KEY] {
            assert_eq!(json[key], sealed[key], "`{key}` travels unchanged");
        }
        assert_eq!(identify(&json).attempt, Some(2));
    }

    /// A record the delivery could not continue a trace from carries the trace
    /// its first attempt ran in, and a value that was no envelope becomes one.
    #[test]
    fn a_retried_record_without_a_trace_joins_the_first_attempts() {
        let first = Correlation::minted(Some("bob"));
        let id = JobId::mint();
        for stored in [
            json!({ "clip": 1 }),
            json!({ "v": 1, "payload": { "clip": 1 } }),
        ] {
            let json = retry(&stored, &id, 2, Some(&first)).into_json();
            assert_eq!(
                json[TRACEPARENT],
                json!(first.traceparent().to_string()),
                "{stored}"
            );
            assert_eq!(json[ACTOR_ID], json!("bob"));
            let Opened::Sealed { payload, .. } = opened(json.clone()) else {
                panic!("{json} is an envelope");
            };
            assert_eq!(payload, json!({ "clip": 1 }), "the payload is kept whole");
        }
    }

    /// The actor is an audit identity, not a trace: an envelope naming one beside
    /// a `traceparent` that is missing or corrupt starts a trace of its own and
    /// keeps the actor, rather than running anonymous where the envelope said
    /// who it was for.
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

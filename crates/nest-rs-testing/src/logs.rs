//! Assert on what the framework *said*, not only on what it returned.
//!
//! A whole class of defect is invisible to a response assertion: a denial that
//! fails closed but logs nothing, a dead-lettered job with no event, a warning
//! filed under the wrong target. Those are the lines an operator queries during
//! an incident, so they deserve the same coverage as a status code.
//!
//! ```no_run
//! # use nest_rs_testing::LogCapture;
//! let logs = LogCapture::install();
//! tracing::warn!(target: "nest_rs::orm", entity = "post", "denying all rows");
//! let event = logs.expect_one("nest_rs::orm", "denying all rows");
//! assert_eq!(event.field("entity").as_deref(), Some("post"));
//! ```
//!
//! The capture is **thread-local** ([`tracing::subscriber::set_default`]), so
//! parallel tests do not see each other's events. Hold the [`LogCapture`] across
//! `.await` points only on a current-thread runtime — which is what
//! `#[tokio::test]` gives you by default.
//!
//! That thread-locality is blind in one place, and it is not a corner: an event
//! the framework emits from a task it *spawned* — a `spawn_blocking` write, a
//! socket's writer half — never runs on the test's thread. Reach for
//! [`LogCapture::install_global`] there.
//!
//! # Spans are captured too, and they carry the contract
//!
//! Most of what the framework promises an operator lives on the **operation
//! span**, not on an event: `trace_id`, `span_id`, `actor_id`, `http.route`,
//! `otel.name`. A harness that could only read events could assert none of it,
//! which is why [`spans`](LogCapture::spans) exists — and why fields recorded
//! *after* a span opens are captured as well as the ones declared at creation.
//! That second half is the whole point at an HTTP edge, where the route template
//! and the status are only known once the inner tree has answered.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::Registry;

/// One recorded `tracing` event.
#[derive(Clone, Debug)]
pub struct CapturedEvent {
    /// The event's target — `nest_rs::orm`, `features::users`, …
    pub target: String,
    /// The event's `name:` — its metadata identity, which an OTLP log bridge
    /// exports as `event.name`.
    ///
    /// A unit of work is named three times (`CLAUDE.md`): by the
    /// `operation_span!` that opens it, by the operation line's `name:`, and by
    /// that line's `message`. Without this field a harness could match only the
    /// message, so a line whose `name:` had drifted from it passed every
    /// assertion in the repo. `tracing` defaults it to `event <file>:<line>`
    /// where the macro states none.
    pub name: String,
    /// The event's level, as its lowercase name (`warn`, `debug`, …).
    pub level: String,
    /// The `message` field: the constant event name, never interpolated data.
    pub message: String,
    /// Every other field, formatted with `Debug` (so a `%`/`?` value reads the
    /// way it does in the JSON output, minus the quoting).
    pub fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// One structured field, if the event carries it.
    pub fn field(&self, name: &str) -> Option<String> {
        self.fields.get(name).cloned()
    }
}

/// One recorded `tracing` span, with every field value it ever held.
#[derive(Clone, Debug)]
pub struct CapturedSpan {
    /// The span's target — `nest_rs::http`, `nest_rs::ws`, …
    pub target: String,
    /// The span's level, as its lowercase name — the *Level per layer* contract
    /// was assertable for events and not for spans.
    pub level: String,
    /// The span's *name* as `tracing` fixes it (`http.request`), which is a
    /// literal and never the exported OTel name — that one is the `otel.name`
    /// field, because `tracing` cannot vary a name per instance.
    pub name: String,
    /// Fields declared at creation **and** recorded afterwards, last write
    /// winning. A field declared `Empty` and never filled is absent, which is
    /// exactly the silent no-op worth asserting against.
    pub fields: BTreeMap<String, String>,
}

impl CapturedSpan {
    /// One field, if the span carries it.
    pub fn field(&self, name: &str) -> Option<String> {
        self.fields.get(name).cloned()
    }
}

/// A live capture of everything logged on this thread until it is dropped.
pub struct LogCapture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
    spans: Arc<Mutex<Vec<CapturedSpan>>>,
    /// `None` for a global capture, which cannot be uninstalled.
    _guard: Option<DefaultGuard>,
}

impl LogCapture {
    /// Start capturing on the current thread. Capturing stops when the returned
    /// value is dropped, restoring whatever subscriber was default before.
    pub fn install() -> Self {
        let capture = Self::empty();
        let guard = tracing::subscriber::set_default(capture.subscriber());
        Self {
            _guard: Some(guard),
            ..capture
        }
    }

    /// Start capturing on **every** thread of this process, for the rest of it.
    ///
    /// [`install`](Self::install) is thread-local, which is what keeps parallel
    /// tests from reading each other's events — and what makes it blind to the
    /// events a framework emits from a task it spawned. Those are not a corner
    /// case: a background write and a socket's writer half are exactly the
    /// failures nothing else reports, so they are the ones most worth asserting.
    ///
    /// Sound here because **nextest runs each test in its own process**, so a
    /// global subscriber cannot leak into another test. It is also permanent —
    /// `tracing` allows one global default per process and offers no way back.
    ///
    /// Three consequences, and the first two are the ones that bite:
    ///
    /// - **Call it before anything boots an app.** `App::builder().build()`
    ///   installs `tracing`'s console fallback, which takes the one global slot;
    ///   after that this panics. It is the first statement of a test, not a line
    ///   near the assertion.
    /// - **Never beside [`install`](Self::install).** A thread-local default
    ///   shadows the global one for that thread, silently: the global handle
    ///   then sees nothing, a positive assertion fails with the confusing
    ///   `captured: []`, and a *negative* one — `assert!(logs.find(..)
    ///   .is_empty())` — passes for the wrong reason. Nothing can distinguish
    ///   "shadowed" from "quiet", which is why this is a rule rather than a
    ///   check.
    /// - **Reach for it only when the events really are off-thread.**
    ///   `#[tokio::test]` is a *current-thread* runtime, so a spawned task still
    ///   runs on the test's own thread and [`install`](Self::install) sees it.
    ///   What needs this is a `spawn_blocking`, a `flavor = "multi_thread"`
    ///   test, or a `std::thread`.
    ///
    /// # Panics
    ///
    /// If a global subscriber is already installed — including the one an
    /// `App` boot installs.
    pub fn install_global() -> Self {
        let capture = Self::empty();
        tracing::subscriber::set_global_default(capture.subscriber()).expect(
            "no global subscriber is installed yet — `LogCapture::install_global` must come \
             before anything that boots an `App`, which installs tracing's console fallback",
        );
        capture
    }

    fn empty() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            spans: Arc::new(Mutex::new(Vec::new())),
            _guard: None,
        }
    }

    fn subscriber(&self) -> impl tracing::Subscriber {
        Registry::default().with(CollectLayer {
            events: Arc::clone(&self.events),
            spans: Arc::clone(&self.spans),
        })
    }

    /// Everything captured so far, in emission order.
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Every event on `target` whose message is exactly `message`.
    pub fn find(&self, target: &str, message: &str) -> Vec<CapturedEvent> {
        self.events()
            .into_iter()
            .filter(|e| e.target == target && e.message == message)
            .collect()
    }

    /// The single event on `target` with `message`, or a panic naming what was
    /// captured instead — a wrong target is the failure this exists to catch,
    /// so the diagnostic has to show the targets that did fire.
    #[track_caller]
    pub fn expect_one(&self, target: &str, message: &str) -> CapturedEvent {
        let mut hits = self.find(target, message);
        assert_eq!(
            hits.len(),
            1,
            "expected exactly one `{message}` on `{target}`, captured: {:#?}",
            self.events(),
        );
        hits.remove(0)
    }

    /// Every span captured so far, in creation order.
    pub fn spans(&self) -> Vec<CapturedSpan> {
        self.spans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The single span on `target` named `name`, or a panic naming what was
    /// captured instead.
    ///
    /// Reach for this to assert the fields an operator actually queries —
    /// `trace_id`, `actor_id`, `http.route`. A field declared `Empty` and never
    /// recorded is **absent** here, which is what makes a `record` call nobody
    /// wired assertable at all: `tracing` fixes a span's fields at creation, so
    /// such a call is a silent no-op and nothing else in a test can see it.
    #[track_caller]
    pub fn expect_span(&self, target: &str, name: &str) -> CapturedSpan {
        let mut hits: Vec<_> = self
            .spans()
            .into_iter()
            .filter(|span| span.target == target && span.name == name)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "expected exactly one `{name}` span on `{target}`, captured: {:#?}",
            self.spans(),
        );
        hits.remove(0)
    }

    /// Assert nothing on `target` carried `message` — the quiet half, and the
    /// one worth wording once.
    ///
    /// A negative assertion is the easiest to write and the easiest to write
    /// uselessly: it passes when the event is absent, and equally when the
    /// target is misspelt, the capture is shadowed, or nothing ran at all. So
    /// the diagnostic has to show what *was* captured, and that is the half a
    /// hand-written `assert!(logs.find(..).is_empty())` keeps forgetting.
    #[track_caller]
    pub fn expect_none(&self, target: &str, message: &str) {
        let hits = self.find(target, message);
        assert!(
            hits.is_empty(),
            "expected no `{message}` on `{target}`, captured: {:#?}",
            self.events(),
        );
    }
}

struct CollectLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
    spans: Arc<Mutex<Vec<CapturedSpan>>>,
}

impl<S> Layer<S> for CollectLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::Id,
        ctx: Context<'_, S>,
    ) {
        let mut visitor = FieldVisitor::default();
        attrs.record(&mut visitor);
        let meta = attrs.metadata();
        let mut spans = self.spans.lock().unwrap_or_else(PoisonError::into_inner);
        spans.push(CapturedSpan {
            target: meta.target().to_string(),
            level: meta.level().as_str().to_lowercase(),
            name: meta.name().to_string(),
            fields: visitor.fields,
        });
        // The buffer entry *is* the storage — a later `record` writes straight
        // into it. The index rides the registry's own per-span extensions rather
        // than a map keyed by id, because an id is reused once a span closes and
        // a map would merge two unrelated spans under one entry.
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanIndex(spans.len() - 1));
        }
    }

    fn on_record(&self, id: &tracing::Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let Some(SpanIndex(index)) = span.extensions().get::<SpanIndex>().copied() else {
            return;
        };
        let mut visitor = FieldVisitor::default();
        values.record(&mut visitor);
        if let Some(captured) = self
            .spans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(index)
        {
            captured.fields.extend(visitor.fields);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(CapturedEvent {
                target: meta.target().to_string(),
                name: meta.name().to_string(),
                level: meta.level().as_str().to_lowercase(),
                message: visitor.message.unwrap_or_default(),
                fields: visitor.fields,
            });
    }
}

/// Which entry in the capture buffer a span writes into.
#[derive(Clone, Copy)]
struct SpanIndex(usize);

#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl FieldVisitor {
    fn put(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = Some(value);
        } else {
            self.fields.insert(field.name().to_string(), value);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.put(field, format!("{value:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_target_level_message_and_fields() {
        let logs = LogCapture::install();
        // A literal, and it must stay one: this is a *fixture* standing in for
        // an ORM event, not an emission. `nest-rs-testing` does not depend on
        // `nest-rs-seaorm` and should not grow the dependency to spell a string
        // the capture is only ever asked to match verbatim.
        tracing::warn!(target: "nest_rs::orm", entity = "post", action = 3, "denying all rows");
        tracing::debug!(target: "nest_rs::orm", "listing rows");

        let event = logs.expect_one("nest_rs::orm", "denying all rows");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("entity").as_deref(), Some("post"));
        assert_eq!(event.field("action").as_deref(), Some("3"));
        // The message is the constant event name; data stays in fields.
        assert!(!event.message.contains("post"));
        assert_eq!(logs.find("nest_rs::orm", "listing rows").len(), 1);
        assert!(logs.find("nest_rs::http", "denying all rows").is_empty());
    }
}

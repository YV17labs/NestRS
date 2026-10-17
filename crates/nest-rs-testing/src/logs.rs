//! Assert on what the framework *said*, not only on what it returned.
//!
//! ```
//! # use nest_rs_testing::LogCapture;
//! # const TARGET: &str = "features::orders";
//! let logs = LogCapture::install();
//! tracing::warn!(target: TARGET, order = 7, "order refused");
//! let event = logs.expect_one(TARGET, "order refused");
//! assert_eq!(event.field("order").as_deref(), Some("7"));
//! ```
//!
//! Read the target from the constant the code under test logs on, never a
//! retyped literal.
//!
//! The capture is **thread-local** ([`tracing::subscriber::set_default`]): hold
//! the [`LogCapture`] across `.await` points only on a current-thread runtime,
//! and reach for [`LogCapture::install_global`] for events emitted off the
//! test's thread (a `spawn_blocking` write, a socket's writer half).
//!
//! [`spans`](LogCapture::spans) captures spans too, with fields recorded after
//! creation as well as those declared at it.

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
    /// The event's `name:`, which an OTLP log bridge exports as `event.name`;
    /// `tracing` defaults it to `event <file>:<line>`.
    pub name: String,
    /// The event's level, as its lowercase name (`warn`, `debug`, …).
    pub level: String,
    /// The `message` field: the constant event name, never interpolated data.
    pub message: String,
    /// Every other field, formatted with `Debug` (so a `%`/`?` value reads the
    /// way it does in the JSON output, minus the quoting).
    pub fields: BTreeMap<String, String>,
    /// The `trace_id` the line renders, read off the ambient context as the
    /// formatters read it; `None` outside a unit of work. Never in `fields`.
    pub trace_id: Option<String>,
    /// The `span_id` the line renders, read as [`trace_id`](Self::trace_id) is.
    pub span_id: Option<String>,
    /// The `actor_id` the line renders, read as [`trace_id`](Self::trace_id) is.
    pub actor_id: Option<String>,
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
    /// The span's level, as its lowercase name.
    pub level: String,
    /// The span's *name* as `tracing` fixes it (`http.request`); the exported
    /// OTel name is the `otel.name` field.
    pub name: String,
    /// Fields declared at creation **and** recorded afterwards, last write
    /// winning; a field declared `Empty` and never filled is absent.
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

    /// Start capturing on **every** thread of this process, for the rest of it
    /// (sound because nextest runs each test in its own process).
    ///
    /// - **Call it before anything boots an app**: `App::builder().build()`
    ///   takes the one global slot, and this then panics.
    /// - **Never beside [`install`](Self::install)**: a thread-local default
    ///   silently shadows the global one, so a negative assertion passes for the
    ///   wrong reason.
    /// - Only for events off the test's thread: a `spawn_blocking`, a
    ///   `flavor = "multi_thread"` test, a `std::thread`.
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
    /// captured instead.
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

    /// Assert nothing on `target` carried `message`, printing what was captured
    /// on failure.
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
        // Not a map keyed by id: an id is reused once a span closes, which would
        // merge two unrelated spans.
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
        let correlation = nest_rs_core::__private::current_correlation();
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(CapturedEvent {
                target: meta.target().to_string(),
                name: meta.name().to_string(),
                level: meta.level().as_str().to_lowercase(),
                message: visitor.message.unwrap_or_default(),
                fields: visitor.fields,
                trace_id: correlation.as_ref().map(|c| c.trace_id().to_string()),
                span_id: correlation.as_ref().map(|c| c.span_id().to_string()),
                actor_id: correlation
                    .as_ref()
                    .and_then(|c| c.actor_id().map(str::to_owned)),
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
        tracing::warn!(target: "nest_rs::orm", entity = "post", action = 3, "denying all rows");
        tracing::debug!(target: "nest_rs::orm", "listing rows");

        let event = logs.expect_one("nest_rs::orm", "denying all rows");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("entity").as_deref(), Some("post"));
        assert_eq!(event.field("action").as_deref(), Some("3"));
        assert!(!event.message.contains("post"));
        assert_eq!(logs.find("nest_rs::orm", "listing rows").len(), 1);
        assert!(logs.find("nest_rs::http", "denying all rows").is_empty());
    }

    #[tokio::test]
    async fn an_event_carries_the_trace_context_it_was_filed_under() {
        let logs = LogCapture::install();
        tracing::info!(target: "nest_rs::orm", entity = "post", "listing rows");
        let correlation = nest_rs_core::Correlation::minted(Some("user-7"));
        let (trace_id, span_id) = (
            correlation.trace_id().to_string(),
            correlation.span_id().to_string(),
        );
        nest_rs_core::with_request_scope(None, correlation, async {
            tracing::warn!(target: "nest_rs::orm", entity = "post", "denying all rows");
        })
        .await;

        let outside = logs.expect_one("nest_rs::orm", "listing rows");
        assert_eq!(
            (outside.trace_id, outside.span_id, outside.actor_id),
            (None, None, None)
        );
        let inside = logs.expect_one("nest_rs::orm", "denying all rows");
        assert_eq!(inside.trace_id, Some(trace_id));
        assert_eq!(inside.span_id, Some(span_id));
        assert_eq!(inside.actor_id.as_deref(), Some("user-7"));
        assert!(
            !inside.fields.contains_key("trace_id"),
            "the ids ride beside the fields, never inside them: {inside:#?}",
        );
    }
}

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

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

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

/// A live capture of everything logged on this thread until it is dropped.
pub struct LogCapture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
    _guard: DefaultGuard,
}

impl LogCapture {
    /// Start capturing on the current thread. Capturing stops when the returned
    /// value is dropped, restoring whatever subscriber was default before.
    pub fn install() -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let layer = CollectLayer {
            events: Arc::clone(&events),
        };
        let guard = tracing::subscriber::set_default(Registry::default().with(layer));
        Self {
            events,
            _guard: guard,
        }
    }

    /// Everything captured so far, in emission order.
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.events.lock().expect("log buffer").clone()
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
}

struct CollectLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CollectLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        self.events.lock().expect("log buffer").push(CapturedEvent {
            target: meta.target().to_string(),
            level: meta.level().as_str().to_lowercase(),
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
        });
    }
}

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
        // The message is the constant event name; data stays in fields.
        assert!(!event.message.contains("post"));
        assert_eq!(logs.find("nest_rs::orm", "listing rows").len(), 1);
        assert!(logs.find("nest_rs::http", "denying all rows").is_empty());
    }
}

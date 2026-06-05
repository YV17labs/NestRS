use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

/// One Server-Timing entry: name, optional `desc`, duration.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub desc: Option<String>,
    pub dur: Duration,
}

/// Per-request accumulator. Pull it from request extensions to record sub-step
/// durations. The interceptor always appends a final `total;dur=X`, so calling
/// `record` is optional.
#[derive(Clone, Default)]
pub struct Timings {
    inner: Arc<Mutex<Vec<Entry>>>,
}

impl Timings {
    pub fn record(&self, name: impl Into<String>, dur: Duration) {
        self.push(name, None, dur);
    }

    /// `desc` disambiguates entries sharing a name in DevTools (rendered as a
    /// tooltip).
    pub fn record_with_desc(
        &self,
        name: impl Into<String>,
        desc: impl Into<String>,
        dur: Duration,
    ) {
        self.push(name, Some(desc.into()), dur);
    }

    fn push(&self, name: impl Into<String>, desc: Option<String>, dur: Duration) {
        self.inner.lock().push(Entry {
            name: name.into(),
            desc,
            dur,
        });
    }

    pub(crate) fn drain(&self) -> Vec<Entry> {
        std::mem::take(&mut *self.inner.lock())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_appends_an_entry_without_a_description() {
        let t = Timings::default();
        t.record("db", Duration::from_millis(7));

        let entries = t.drain();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "db");
        assert!(entries[0].desc.is_none());
        assert_eq!(entries[0].dur, Duration::from_millis(7));
    }

    #[test]
    fn record_with_desc_preserves_the_label() {
        let t = Timings::default();
        t.record_with_desc("cache", "warm", Duration::from_micros(50));

        let entries = t.drain();
        assert_eq!(entries[0].desc.as_deref(), Some("warm"));
    }

    #[test]
    fn drain_returns_in_recording_order_and_resets_state() {
        let t = Timings::default();
        t.record("a", Duration::from_millis(1));
        t.record("b", Duration::from_millis(2));
        t.record("c", Duration::from_millis(3));

        let first = t.drain();
        assert_eq!(
            first.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"],
        );
        // After drain, the accumulator is empty — a second drain yields nothing.
        assert!(t.drain().is_empty(), "drain must reset");
    }

    #[test]
    fn clone_shares_the_underlying_accumulator() {
        // Pulled from request extensions, the `Timings` is cloned to each
        // recorder — both writers must hit the same buffer the interceptor
        // drains at end-of-handler.
        let a = Timings::default();
        let b = a.clone();
        a.record("from-a", Duration::from_millis(1));
        b.record("from-b", Duration::from_millis(2));

        let entries = a.drain();
        assert_eq!(entries.len(), 2);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"from-a"));
        assert!(names.contains(&"from-b"));
    }
}

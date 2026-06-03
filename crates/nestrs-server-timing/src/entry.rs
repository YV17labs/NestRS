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

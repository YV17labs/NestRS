use std::time::Duration;

/// A listener whose body did not end within its timeout, and was cut there: its
/// unit of work rolled back, so nothing it wrote stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListenerTimedOut {
    /// The timeout the listener ran past.
    timeout: Duration,
}

impl ListenerTimedOut {
    /// The listener's body ran past `timeout`.
    pub(crate) const fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl std::fmt::Display for ListenerTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the listener did not end within its {:?} timeout, and was cut — \
             `#[on_event(timeout = …)]` sets it",
            self.timeout
        )
    }
}

impl std::error::Error for ListenerTimedOut {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentence_names_the_bound_and_the_key_that_sets_it() {
        assert_eq!(
            ListenerTimedOut::new(Duration::from_secs(30)).to_string(),
            "the listener did not end within its 30s timeout, and was cut — \
             `#[on_event(timeout = …)]` sets it",
        );
    }
}

//! `#[listeners]` + `#[on_event]`.

use nest_rs_core::injectable;
use nest_rs_events::listeners;

/// A bus event — plain `Clone` struct, no serde.
#[derive(Clone)]
pub struct HygieneEvent {
    /// Payload proving field access in the handler compiles.
    pub label: &'static str,
}

/// Minimal listener host.
#[injectable]
pub struct HygieneListener;

#[listeners]
impl HygieneListener {
    /// Handler consuming the event by value.
    #[on_event]
    async fn on_hygiene(&self, event: HygieneEvent) {
        let _ = event.label;
    }
}

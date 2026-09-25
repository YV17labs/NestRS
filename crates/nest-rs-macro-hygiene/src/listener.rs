//! `#[listeners]` + `#[on_event]`.

use nest_rs::core::injectable;
use nest_rs::events::listeners;

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

    /// A synchronous listener is called without an `.await`.
    #[on_event]
    fn on_hygiene_sync(&self, event: HygieneEvent) {
        let _ = event.label;
    }

    /// A listener compiled out takes its wiring with it.
    #[cfg(any())]
    #[on_event]
    async fn compiled_out(&self, event: crate::does_not_exist::Event) {
        crate::does_not_exist::handle(event)
    }
}

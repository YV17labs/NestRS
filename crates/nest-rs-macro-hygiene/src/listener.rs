//! `#[listeners]` + `#[on_event]`, with the layers a listener call takes on its
//! block and its method.

use nest_rs::core::injectable;
use nest_rs::events::{EventsView, listeners};

use crate::pipeline::{HygieneUnitAudit, HygieneUnitRefusal};

/// A bus event: plain `Clone`, no serde.
#[derive(Clone)]
pub struct HygieneEvent {
    pub label: &'static str,
}

/// A listener's own error, converting into the boxed error the bus reports.
#[derive(Debug)]
pub struct HygieneRefusal;

impl std::fmt::Display for HygieneRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the hygiene listener refused")
    }
}

impl std::error::Error for HygieneRefusal {}

#[injectable]
pub struct HygieneListener;

/// A value `#[meta]` declares, read by type.
pub struct HygieneLimit(pub u32);

/// `EventsView`'s path through the umbrella.
pub fn listener_of(view: &EventsView) -> &'static str {
    view.listener()
}

#[listeners]
#[meta(HygieneLimit(100))]
#[use_interceptors(HygieneUnitAudit)]
#[use_exception_filters(HygieneUnitRefusal)]
impl HygieneListener {
    #[on_event]
    #[meta(HygieneLimit(10))]
    #[use_interceptors(HygieneUnitAudit)]
    #[use_exception_filters(HygieneUnitRefusal)]
    async fn on_hygiene(&self, event: HygieneEvent) {
        let _ = event.label;
    }

    #[on_event]
    fn on_hygiene_sync(&self, event: HygieneEvent) {
        let _ = event.label;
    }

    #[on_event(transactional = false, timeout = "30s")]
    async fn on_hygiene_fallible(&self, event: HygieneEvent) -> Result<(), HygieneRefusal> {
        if event.label.is_empty() {
            return Err(HygieneRefusal);
        }
        Ok(())
    }

    /// A listener compiled out takes its wiring with it.
    #[cfg(any())]
    #[on_event]
    async fn compiled_out(&self, event: crate::does_not_exist::Event) {
        crate::does_not_exist::handle(event)
    }
}

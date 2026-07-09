//! Consumer-only slice: reacts to facts other features publish on the event
//! bus. No port of its own — it owns only the `events/` listener adapter.
pub mod events;

pub use events::{NotificationsEventsModule, NotificationsListener};

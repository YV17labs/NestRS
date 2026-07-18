//! Notifications slice: on a `PostPublishedEvent`, the `events/` listener
//! enqueues a `NotifyCommand` (producer only — a listener has no request
//! context, so it never touches the DB); the `queue/` worker consumes it and
//! persists a `Notification`; the `http/` adapter exposes that append-only log
//! as a **read-only**, org-scoped resource.
mod command;
mod entity;
mod module;
mod service;

pub mod events;
pub mod http;
pub mod queue;

pub use command::{NOTIFICATIONS_QUEUE, NotifyCommand, NotifyQueue};
pub use entity::*;
pub use module::NotificationsModule;
pub use service::*;

pub use events::{NotificationsEventsModule, NotificationsListener};
pub use http::{NotificationsController, NotificationsHttpModule};
pub use queue::{NotificationsProcessor, NotificationsQueueModule};

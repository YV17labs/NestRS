use nest_rs_queue::{QueueName, queue};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Imperative payload for the `notifications` queue — "record this notification".
/// A **`Command`** (one processor persists it), the producer↔worker contract:
/// the event listener pushes it and the worker's [`super::NotificationsProcessor`]
/// drains it, so it lives at the feature port. It carries the `org_id` the
/// notification is scoped to plus the human-readable `message`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyCommand {
    pub org_id: Uuid,
    pub message: String,
}

/// Typed handle for the `notifications` queue — the single artifact both sides
/// import. The producer (the event listener) pushes with `push_to::<NotifyQueue>`
/// and the consumer ([`super::NotificationsProcessor`]) drains with
/// `#[process(queue = NotifyQueue)]`; the name and the [`NotifyCommand`] payload
/// are compile-checked on both ends.
#[queue(name = "notifications", job = NotifyCommand)]
pub struct NotifyQueue;

/// The queue's wire name, sourced from [`NotifyQueue`] so the literal
/// `"notifications"` lives in exactly one place.
pub const NOTIFICATIONS_QUEUE: &str = <NotifyQueue as QueueName>::NAME;

pub const TARGET: &str = "features::notifications";

mod command;
mod entity;
mod module;
mod service;

pub mod events;
pub mod http;
pub mod queue;
pub mod schedule;
pub mod ws;

pub use command::{NotifyCommand, NotifyQueue};
pub use entity::*;
pub use module::NotificationsModule;
pub use service::*;

pub use events::NotificationsEventsModule;
pub use http::NotificationsHttpModule;
pub use queue::NotificationsQueueModule;
pub use schedule::NotificationsScheduleModule;
pub use ws::{NotificationsNs, NotificationsWsModule};

use nest_rs::core::module;
use nest_rs::events::EventsModule;

use super::listener::NotificationsListener;

#[module(
    imports = [EventsModule],
    providers = [NotificationsListener],
)]
pub struct NotificationsEventsModule;

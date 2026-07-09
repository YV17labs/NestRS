use nest_rs_core::module;
use nest_rs_events::EventsModule;

use super::listener::NotificationsListener;

#[module(
    imports = [EventsModule],
    providers = [NotificationsListener],
)]
pub struct NotificationsEventsModule;

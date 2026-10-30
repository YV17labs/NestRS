use nest_rs::core::module;
use nest_rs::schedule::ScheduleModule;

use super::tasks::NotificationsTasks;
use crate::notifications::NotificationsModule;

#[module(imports = [NotificationsModule, ScheduleModule], providers = [NotificationsTasks])]
pub struct NotificationsScheduleModule;

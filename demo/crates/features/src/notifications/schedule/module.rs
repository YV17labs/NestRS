use nest_rs::core::module;

use super::tasks::NotificationsTasks;
use crate::notifications::NotificationsModule;

#[module(imports = [NotificationsModule], providers = [NotificationsTasks])]
pub struct NotificationsScheduleModule;

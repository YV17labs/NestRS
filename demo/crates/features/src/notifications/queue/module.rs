use nest_rs::core::module;

use super::processor::NotificationsProcessor;
use crate::notifications::NotificationsModule;

#[module(imports = [NotificationsModule], providers = [NotificationsProcessor])]
pub struct NotificationsQueueModule;

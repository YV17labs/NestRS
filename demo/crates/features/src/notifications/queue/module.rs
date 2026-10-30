use nest_rs::core::module;
use nest_rs::queue::QueueModule;

use super::processor::NotificationsProcessor;
use crate::notifications::NotificationsModule;

#[module(imports = [NotificationsModule, QueueModule], providers = [NotificationsProcessor])]
pub struct NotificationsQueueModule;

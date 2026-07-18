use nest_rs_core::module;

use super::processor::NotificationsProcessor;
use crate::notifications::NotificationsModule;

#[module(imports = [NotificationsModule], providers = [NotificationsProcessor])]
pub struct NotificationsQueueModule;

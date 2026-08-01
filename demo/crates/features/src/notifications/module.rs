use nest_rs::core::module;

use super::service::NotificationsService;

#[module(providers = [NotificationsService])]
pub struct NotificationsModule;

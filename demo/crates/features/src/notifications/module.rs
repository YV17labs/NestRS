use nest_rs_core::module;

use super::service::NotificationsService;

#[module(providers = [NotificationsService])]
pub struct NotificationsModule;

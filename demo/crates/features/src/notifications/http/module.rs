use nest_rs::core::module;

use super::controller::NotificationsController;
use crate::app_authz::AppAuthzHttpModule;
use crate::notifications::NotificationsModule;

#[module(
    imports = [NotificationsModule, AppAuthzHttpModule],
    providers = [NotificationsController],
)]
pub struct NotificationsHttpModule;

use nest_rs::core::module;

use super::controller::NotificationsController;
use crate::authz::AuthzModule;
use crate::notifications::NotificationsModule;

#[module(
    imports = [NotificationsModule, AuthzModule],
    providers = [NotificationsController],
)]
pub struct NotificationsHttpModule;

use nest_rs_core::module;

use super::controller::NotificationsController;
use crate::authz::AuthzHttpModule;
use crate::notifications::NotificationsModule;

#[module(
    imports = [NotificationsModule, AuthzHttpModule],
    providers = [NotificationsController],
)]
pub struct NotificationsHttpModule;

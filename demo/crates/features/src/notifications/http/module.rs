use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::controller::NotificationsController;
use crate::authz::AuthzModule;
use crate::notifications::NotificationsModule;

#[module(
    imports = [NotificationsModule, AuthzModule, HttpModule],
    providers = [NotificationsController],
)]
pub struct NotificationsHttpModule;

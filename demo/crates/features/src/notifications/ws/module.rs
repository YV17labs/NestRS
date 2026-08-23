use nest_rs::core::module;
use nest_rs::ws::WsModule;

use super::gateway::NotificationsGateway;
use crate::authn::AuthnModule;

#[module(imports = [AuthnModule, WsModule], providers = [NotificationsGateway])]
pub struct NotificationsWsModule;

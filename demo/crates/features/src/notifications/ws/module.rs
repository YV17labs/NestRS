use nest_rs::core::module;
use nest_rs::http::HttpModule;
use nest_rs::ws::WsModule;

use super::gateway::NotificationsGateway;
use crate::authn::AuthnModule;

#[module(imports = [AuthnModule, HttpModule, WsModule], providers = [NotificationsGateway])]
pub struct NotificationsWsModule;

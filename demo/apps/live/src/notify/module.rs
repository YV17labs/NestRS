use features::authn::AuthnModule;
use nest_rs::core::module;
use nest_rs::ws::WsModule;

use crate::notify::gateway::NotifyGateway;

#[module(imports = [AuthnModule, WsModule], providers = [NotifyGateway])]
pub struct NotifyModule;

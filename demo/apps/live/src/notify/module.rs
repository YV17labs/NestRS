use features::app_authn::AppAuthnModule;
use nest_rs::core::module;
use nest_rs::ws::WsModule;

use crate::notify::gateway::NotifyGateway;

#[module(imports = [AppAuthnModule, WsModule], providers = [NotifyGateway])]
pub struct NotifyModule;

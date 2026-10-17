use nest_rs::core::module;
use nest_rs::ws::WsModule;

use crate::authz::AuthzModule;

#[module(imports = [AuthzModule, WsModule])]
pub struct AuthzWsModule;

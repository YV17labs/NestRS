use nest_rs::core::module;
use nest_rs::http::HttpModule;
use nest_rs::ws::WsModule;

use super::gateway::UsersGateway;
use crate::authz::AuthzModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzModule, HttpModule, WsModule],
    providers = [UsersGateway],
)]
pub struct UsersWsModule;

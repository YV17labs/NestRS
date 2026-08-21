use nest_rs::core::module;

use super::gateway::UsersGateway;
use crate::app_authz::AppAuthzWsModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AppAuthzWsModule],
    providers = [UsersGateway],
)]
pub struct UsersWsModule;

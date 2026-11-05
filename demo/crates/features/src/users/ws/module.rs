use nest_rs::core::module;

use super::gateway::UsersGateway;
use crate::authz::AuthzModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzModule],
    providers = [UsersGateway],
)]
pub struct UsersWsModule;

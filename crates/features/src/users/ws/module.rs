use nest_rs_core::module;

use super::gateway::UsersGateway;
use crate::authz::AuthzWsModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzWsModule],
    providers = [UsersGateway],
)]
pub struct UsersWsModule;

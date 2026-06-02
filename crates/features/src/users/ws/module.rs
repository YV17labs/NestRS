use nestrs_core::module;

use super::gateway::UsersGateway;
use crate::authz::AuthzWsModule;
use crate::users::core::UsersCoreModule;

#[module(
    imports = [UsersCoreModule, AuthzWsModule],
    providers = [UsersGateway],
)]
pub struct UsersWsModule;

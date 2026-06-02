use nestrs_core::module;
use nestrs_ws::WsModule;

use crate::users::controller::UsersController;
use crate::users::gateway::UsersGateway;
use crate::users::resolver::UsersResolver;

#[module(
    imports = [domain::users::UsersModule, WsModule, domain::authz::AuthzModule],
    providers = [UsersController, UsersGateway, UsersResolver]
)]
pub struct UsersModule;

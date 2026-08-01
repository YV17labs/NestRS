use nest_rs::core::module;

use super::controller::UsersController;
use crate::authz::AuthzHttpModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzHttpModule],
    providers = [UsersController],
)]
pub struct UsersHttpModule;

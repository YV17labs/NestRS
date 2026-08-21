use nest_rs::core::module;

use super::controller::UsersController;
use crate::app_authz::AppAuthzHttpModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AppAuthzHttpModule],
    providers = [UsersController],
)]
pub struct UsersHttpModule;

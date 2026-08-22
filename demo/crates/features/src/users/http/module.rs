use nest_rs::core::module;

use super::controller::UsersController;
use crate::authz::AuthzModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzModule],
    providers = [UsersController],
)]
pub struct UsersHttpModule;

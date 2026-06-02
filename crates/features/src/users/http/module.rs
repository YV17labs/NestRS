use nestrs_core::module;

use super::controller::UsersController;
use crate::authz::AuthzHttpModule;
use crate::users::core::UsersCoreModule;

#[module(
    imports = [UsersCoreModule, AuthzHttpModule],
    providers = [UsersController],
)]
pub struct UsersHttpModule;

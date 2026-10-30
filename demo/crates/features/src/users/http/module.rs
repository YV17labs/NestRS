use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::controller::UsersController;
use crate::authz::AuthzModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, AuthzModule, HttpModule],
    providers = [UsersController],
)]
pub struct UsersHttpModule;

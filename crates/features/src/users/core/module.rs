use nestrs_core::module;

use super::service::UsersService;

#[module(providers = [UsersService])]
pub struct UsersCoreModule;

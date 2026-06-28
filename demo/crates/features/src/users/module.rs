use nest_rs_core::module;

use super::service::UsersService;

#[module(providers = [UsersService])]
pub struct UsersModule;

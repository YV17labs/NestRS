use nest_rs::core::module;

use super::service::UsersService;

#[module(providers = [UsersService])]
pub struct UsersModule;

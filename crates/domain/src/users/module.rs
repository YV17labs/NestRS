use nestrs_core::module;

use crate::users::resolver::UsersResolver;
use crate::users::service::UsersService;

#[module(providers = [UsersService, UsersResolver])]
pub struct UsersModule;

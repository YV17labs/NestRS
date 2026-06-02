use nestrs_core::module;

use crate::users::resolver::UserRelations;
use crate::users::service::UsersService;

#[module(providers = [UsersService, UserRelations])]
pub struct UsersModule;

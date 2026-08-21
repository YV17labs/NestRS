use nest_rs::core::module;

use super::resolver::UsersResolver;
use crate::app_authz::graphql::AppAuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, OrgsModule, AppAuthzGraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

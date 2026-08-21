use nest_rs::core::module;

use super::resolver::OrgsResolver;
use crate::app_authz::graphql::AppAuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [OrgsModule, UsersModule, AppAuthzGraphqlModule],
    providers = [OrgsResolver],
)]
pub struct OrgsGraphqlModule;

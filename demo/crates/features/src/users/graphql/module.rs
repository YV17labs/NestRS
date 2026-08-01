use nest_rs::core::module;

use super::resolver::UsersResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, OrgsModule, AuthzGraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

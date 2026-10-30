use nest_rs::core::module;
use nest_rs::graphql::GraphqlModule;

use super::resolver::UsersResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, OrgsModule, AuthzGraphqlModule, GraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

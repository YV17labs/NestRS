use nest_rs::core::module;

use super::resolver::UsersResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::posts::PostsModule;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, OrgsModule, PostsModule, AuthzGraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

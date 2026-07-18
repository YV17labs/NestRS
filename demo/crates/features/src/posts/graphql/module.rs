use nest_rs_core::module;

use super::resolver::PostsResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::posts::PostsModule;
use crate::users::UsersModule;

#[module(
    imports = [PostsModule, OrgsModule, UsersModule, AuthzGraphqlModule],
    providers = [PostsResolver],
)]
pub struct PostsGraphqlModule;

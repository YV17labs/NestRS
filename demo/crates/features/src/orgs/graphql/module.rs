use nest_rs::core::module;

use super::resolver::OrgsResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::posts::PostsModule;
use crate::users::UsersModule;

#[module(
    imports = [OrgsModule, UsersModule, PostsModule, AuthzGraphqlModule],
    providers = [OrgsResolver],
)]
pub struct OrgsGraphqlModule;

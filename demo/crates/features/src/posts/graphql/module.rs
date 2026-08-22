use nest_rs::core::module;

use super::resolver::PostsResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::posts::{PostsEventsModule, PostsModule};
use crate::users::UsersModule;

#[module(
    imports = [PostsModule, PostsEventsModule, OrgsModule, UsersModule, AuthzGraphqlModule],
    providers = [PostsResolver],
)]
pub struct PostsGraphqlModule;

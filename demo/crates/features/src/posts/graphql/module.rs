use nest_rs::core::module;

use super::resolver::PostsResolver;
use crate::app_authz::graphql::AppAuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::posts::{PostsEventsModule, PostsModule};
use crate::users::UsersModule;

#[module(
    imports = [PostsModule, PostsEventsModule, OrgsModule, UsersModule, AppAuthzGraphqlModule],
    providers = [PostsResolver],
)]
pub struct PostsGraphqlModule;

use nest_rs::core::module;
use nest_rs::graphql::GraphqlModule;

use super::resolver::PostsResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::posts::PostsModule;
use crate::users::UsersModule;

#[module(
    imports = [
        PostsModule,
        OrgsModule,
        UsersModule,
        AuthzGraphqlModule,
        GraphqlModule,
    ],
    providers = [PostsResolver],
)]
pub struct PostsGraphqlModule;

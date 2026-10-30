use nest_rs::core::module;
use nest_rs::graphql::GraphqlModule;

use super::resolver::OrgsResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [OrgsModule, UsersModule, AuthzGraphqlModule, GraphqlModule],
    providers = [OrgsResolver],
)]
pub struct OrgsGraphqlModule;

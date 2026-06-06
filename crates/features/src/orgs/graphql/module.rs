use nest_rs_core::module;

use super::resolver::OrgsResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [OrgsModule, UsersModule, AuthzGraphqlModule],
    providers = [OrgsResolver],
)]
pub struct OrgsGraphqlModule;

use nestrs_core::module;

use super::resolver::UsersResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::users::core::UsersCoreModule;

#[module(
    imports = [UsersCoreModule, AuthzGraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

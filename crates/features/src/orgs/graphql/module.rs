use nestrs_core::module;

use super::resolver::OrgsResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::core::OrgsCoreModule;
use crate::users::core::UsersCoreModule;

#[module(
    imports = [OrgsCoreModule, UsersCoreModule, AuthzGraphqlModule],
    providers = [OrgsResolver],
)]
pub struct OrgsGraphqlModule;

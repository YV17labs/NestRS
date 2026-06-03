use nestrs_core::module;

use super::resolver::UsersResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsCoreModule;
use crate::users::core::UsersCoreModule;

/// `OrgsCoreModule` is imported so the `User.org` `#[field]` can resolve
/// `DataLoader<OrgsServiceById>`. The macro deliberately strips loader types
/// from a resolver's `injected_deps` (they belong to GraphQL's per-request
/// pool, not the access graph), so this dependency is otherwise invisible to
/// the boot check — a users-only app would link cleanly and then panic on the
/// first `User.org` query.
#[module(
    imports = [UsersCoreModule, OrgsCoreModule, AuthzGraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

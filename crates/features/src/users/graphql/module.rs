use nest_rs_core::module;

use super::resolver::UsersResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

/// `OrgsModule` is imported for the `User.org` `#[field]` dataloader. The
/// macro strips loader types from `injected_deps` (they live in GraphQL's
/// per-request pool), so without this import a `User.org` query would panic
/// at runtime instead of failing the boot.
#[module(
    imports = [UsersModule, OrgsModule, AuthzGraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

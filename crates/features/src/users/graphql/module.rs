use nest_rs_core::module;

use super::resolver::UsersResolver;
use crate::authz::graphql::AuthzGraphqlModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

/// `OrgsModule` is imported so the auto-emitted `User.org` resolver
/// (`#[expose]` on `users::Entity`) can reach `OrgsServiceById` — the
/// dataloader the macro generates on `OrgsService` for the BelongsTo path.
/// The loader is module-gated on `OrgsService` reachability; without this
/// import the `LoaderExtension` skips its registration and the first
/// `{ users { org { … } } }` query panics inside `data_unchecked` instead
/// of failing the boot.
#[module(
    imports = [UsersModule, OrgsModule, AuthzGraphqlModule],
    providers = [UsersResolver],
)]
pub struct UsersGraphqlModule;

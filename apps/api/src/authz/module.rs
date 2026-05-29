//! `AuthzModule` — the app's authorization wiring, for **both** surfaces. REST
//! routes bind `AppAbilityGuard`; the GraphQL endpoint resolves `GraphqlAuthGuard`
//! as its `OperationGuard`. The rules themselves are in `ability.rs`, the guard
//! aliases in `guard.rs`. (The HTTP/GraphQL split stays at the framework-crate
//! level — `nestrs-authz-http`/`-graphql`; the app needs a single authz module.)

use identity::Claims;
use nestrs_core::module;
use nestrs_graphql::OperationGuard;

use crate::authn::AuthnModule;
use crate::authz::ability::AppAbility;
use crate::authz::guard::{AppAbilityGuard, GraphqlAuthGuard};

#[module(
    imports = [AuthnModule],
    providers = [AppAbility, AppAbilityGuard, GraphqlAuthGuard as dyn OperationGuard],
)]
pub struct AuthzModule;

// Forward the authenticated caller into the GraphQL context, so resolvers read it
// with `ctx.data::<Claims>()` (the ambient `Ability` is forwarded by
// `nestrs-authz-graphql`'s own seed).
nestrs_graphql::forward_principal!(Claims);

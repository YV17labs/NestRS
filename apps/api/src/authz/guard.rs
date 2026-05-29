//! The app's authorization guards — type aliases binding the framework's generic
//! guards to this app's policy ([`AppAbility`](crate::authz::ability::AppAbility))
//! and its authentication guard. Both surfaces in one place; the rules themselves
//! live in `ability.rs`.

use nestrs_authz_graphql::GraphqlAbilityBridge;
use nestrs_authz_http::AbilityGuard;

use crate::authn::AuthGuard;
use crate::authz::ability::AppAbility;

/// REST: builds the caller's `Ability` from `AppAbility` and attaches it. Bind with
/// `#[use_guards(AuthGuard, AppAbilityGuard)]`.
pub type AppAbilityGuard = AbilityGuard<AppAbility>;

/// GraphQL: runs the same `AuthGuard` + `AppAbilityGuard` chain around every
/// operation and installs the ambient ability — mounted `as dyn OperationGuard`.
pub type GraphqlAuthGuard = GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>;

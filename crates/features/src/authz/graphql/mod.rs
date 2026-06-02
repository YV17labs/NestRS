//! Authz GraphQL adapter — three pieces:
//!
//! 1. [`AppGraphqlGuard`] (the `dyn OperationGuard`) — runs the HTTP
//!    `AuthGuard` → `AppAbilityGuard` chain once per GraphQL request and
//!    installs `Arc<Ability>` into the operation's ambient scope.
//! 2. [`GraphqlAuthGuard`] (a `ResolverGuard`) — bound per-resolver via
//!    `#[use_guards(GraphqlAuthGuard)]` so the **access graph** sees that
//!    every feature's GraphQL adapter depends on this module. Without it the
//!    runtime dependency on `Ability` (seeded into the GraphQL context by the
//!    operation guard) would be invisible to the import contract.
//! 3. [`LoaderScope`](nestrs_database::graphql::LoaderScope) (the
//!    `BatchContext`) — re-installs the request's ambient state around each
//!    DataLoader batch so loaders' `Repo` reads stay scoped.
//!
//! Plus the `forward_principal!(Claims)` seed (registered by
//! [`AuthzGraphqlModule`]) that forwards the authenticated principal from
//! the poem request into the GraphQL context.

mod bridge;
mod guard;
mod module;

pub use bridge::AppGraphqlGuard;
pub use guard::GraphqlAuthGuard;
pub use module::AuthzGraphqlModule;

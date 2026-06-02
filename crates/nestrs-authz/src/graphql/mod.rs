//! GraphQL surface for [`nestrs_authz`](crate). Enabled by the `graphql` Cargo feature.
//!
//! The resolver-side analog of [`crate::http`]. What this module exposes:
//!
//! - [`GraphqlAbilityBridge`] — the per-operation guard that authenticates the
//!   request and installs the caller's ambient ability (analog of HTTP's
//!   `AbilityGuard` + `Authorize` shaper); implements `nestrs-graphql`'s
//!   `OperationGuard` seam.
//! - [`authorize`] — the class-level gate (analog of HTTP `Authorize`).
//! - [`ability`] — the per-request `Ability` accessor; the `ContextSeed` that
//!   forwards `Arc<Ability>` into every operation's GraphQL context is
//!   submitted as a side effect of importing this module.
//!
//! The data-coupled bindings live in `nestrs-database::graphql` (behind the
//! `graphql` feature of `nestrs-database`):
//!
//! - `nestrs_database::graphql::bind` — route-model binding by id (analog of
//!   `nestrs_database::Bind`).
//! - `nestrs_database::graphql::LoaderScope` — `BatchContext` implementor that
//!   re-installs the ambient executor + ability inside each `#[dataloader]`
//!   batch.
//!
//! ```ignore
//! use nestrs_authz::Read;
//! use nestrs_authz::graphql::authorize;
//! use nestrs_database::graphql::bind;
//!
//! #[resolver]
//! impl UsersResolver {
//!     #[query]
//!     async fn users(&self, ctx: &Context<'_>) -> Result<Vec<User>> {
//!         authorize::<Read, users::Entity>(ctx)?; // gate; `Repo` then scopes the read
//!         // ...
//!     }
//!     #[query]
//!     async fn user(&self, ctx: &Context<'_>, id: String) -> Result<Option<User>> {
//!         Ok(bind::<UsersService, Read>(ctx, &id).await?.as_ref().map(User::from))
//!     }
//! }
//! ```
//!
//! The caller's `Ability` reaches `/graphql` via the app's GraphQL auth bridge (the
//! guard chain run on that route); without it, `authorize`/`bind` return `FORBIDDEN`.

mod authorize;
mod bridge;
mod context;

pub use authorize::authorize;
pub use bridge::GraphqlAbilityBridge;
pub use context::ability;

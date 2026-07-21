//! Open social-login provider contract for nestrs.
//!
//! Social login is a first-class capability with an **open provider
//! contract**: the framework ships the [`SocialProvider`] trait, an
//! inventory-based [registry](SocialProviders), the base [`SocialModule`], and
//! two first-party providers (GitHub, Google). A third-party developer
//! publishes their own provider as an independent crate that depends on this
//! one, implements [`SocialProvider`], ships a `Module`/`Setup` import pair,
//! and submits one
//! [`SocialProviderEntry`] to the registry — the exact same public seam the
//! first-party providers use (dogfooded, no crate-private shortcut).
//!
//! Discovery is link-time and **module-gated** by
//! [`ReachableProviders`](nest_rs_core::ReachableProviders): a provider whose
//! module the running app did not import stays inert (with a boot `warn`), so
//! each deployment's provider set is a composition decision. A duplicate key,
//! or a registry key that disagrees with the provider's own
//! [`SocialProvider::key`], **fails boot**.
//!
//! The contract is **flow-owning**: [`SocialProvider::authorize`] and
//! [`SocialProvider::exchange`] default to the shared PKCE/CSRF flow, so a
//! standard provider implements only [`SocialProvider::profile`]. A provider
//! with a non-standard protocol overrides a step without changing the trait —
//! the ecosystem never breaks on a new provider shape.
#![warn(missing_docs)]

mod module;
mod provider;
mod registry;

pub mod providers;

pub use module::SocialModule;
pub use provider::{ProfileFuture, SocialProfile, SocialProvider, TokenFuture};
pub use registry::{SocialProviderEntry, SocialProviders};

pub use providers::github::{
    GithubSocialConfig, GithubSocialProvider, GithubSocialProviderModule, GithubSocialProviderSetup,
};
pub use providers::google::{
    GoogleSocialConfig, GoogleSocialProvider, GoogleSocialProviderModule, GoogleSocialProviderSetup,
};

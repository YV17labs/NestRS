//! Open social-login provider contract for nestrs.
//!
//! It ships the [`SocialProvider`] trait, an inventory-based [`SocialRegistry`],
//! the [`SocialModule`] that owns it, and two providers (GitHub, Google), each
//! opened by its own module: an app imports [`GithubSocialModule`] or
//! [`GoogleSocialModule`] beside [`SocialModule`], one visible line per
//! provider. A third-party provider is a crate implementing [`SocialProvider`] +
//! [`SocialProviderConfig`], shipping its module and submitting one
//! [`SocialProviderEntry`] that names it ([`SocialProviderEntry::module`]).
//!
//! Each imported provider reads its own `#[config]` ([`SocialProviderEntry::config_namespace`]):
//!
//! | `<PREFIX>_SOCIAL__<KEY>__*`, over any base the provider's config resolved | Outcome |
//! |---|---|
//! | complete | active |
//! | absent entirely | inert, one boot `warn` — its routes 404 like an unknown key |
//! | partial, or invalid | **boot fails**, naming the provider |
//!
//! A duplicate key, or a registry key that disagrees with the provider's own
//! [`SocialProvider::key`], **fails boot**. [`SocialProvider::authorize`] and
//! [`SocialProvider::exchange`] default to the shared PKCE/CSRF flow, so a
//! standard provider implements only [`SocialProvider::profile`].

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — Discovered social providers and their credential state.
pub const TARGET: &str = "nest_rs::social";

mod module;
mod provider;
mod registry;

pub mod providers;

pub use module::SocialModule;
pub use provider::{ProfileFuture, SocialProfile, SocialProvider, TokenFuture};
pub use registry::{
    BuiltProvider, SocialProviderConfig, SocialProviderEntry, SocialRegistry, resolve_provider,
};

pub use providers::github::{GithubSocialConfig, GithubSocialModule, GithubSocialProvider};
pub use providers::google::{GoogleSocialConfig, GoogleSocialModule, GoogleSocialProvider};

//! Open social-login provider contract for nestrs.
//!
//! Social login is a first-class capability with an **open provider
//! contract**: the framework ships the [`SocialProvider`] trait, an
//! inventory-based [`SocialRegistry`], the base [`SocialModule`], and two
//! first-party providers (GitHub, Google). A third-party developer publishes
//! their own provider as an independent crate that depends on this one,
//! implements [`SocialProvider`] + [`SocialProviderConfig`], and submits one
//! [`SocialProviderEntry`] — the exact same public seam the first-party
//! providers use (dogfooded, no crate-private shortcut).
//!
//! # `SocialModule` is the module gate
//!
//! A social provider is **not** a DI provider: it is never `#[inject]`ed by
//! type, only reached through [`SocialRegistry`] as `Arc<dyn SocialProvider>`.
//! So the module that owns every registry entry is [`SocialModule`], and
//! discovery is module-gated by it exactly like any other concern — no app
//! imports `SocialModule`, no entry is ever considered.
//!
//! Within that gate, what decides a provider's fate is **configuration**, the
//! same dual-path `#[config]` rule every `nest-rs-*` module follows:
//!
//! | `NESTRS_SOCIAL__<KEY>__*` (or a provided [`SocialProviderConfig`]) | Outcome |
//! |---|---|
//! | complete | active |
//! | absent entirely | inert, one boot `warn` — its routes 404 like an unknown key |
//! | partial, or invalid | **boot fails**, naming the provider |
//!
//! Real credentials are a deployment's explicit intent, so activation never
//! happens by accident, and a half-configured login is never silently dropped.
//! Pinning config in code is the ordinary config seam — provide the
//! `GithubSocialConfig` value; it wins over the environment. A duplicate key,
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
pub use registry::{
    BuiltProvider, SocialProviderConfig, SocialProviderEntry, SocialRegistry, resolve_provider,
};

pub use providers::github::{GithubSocialConfig, GithubSocialProvider};
pub use providers::google::{GoogleSocialConfig, GoogleSocialProvider};

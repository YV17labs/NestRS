//! First-party providers. Each folder is the template a third-party provider
//! crate copies. Two files are the whole contract:
//!
//! - `config.rs` — the dual-path `#[config]` type plus its
//!   [`SocialProviderConfig`](crate::SocialProviderConfig) impl, which decides
//!   *unconfigured* (inert) from *partially configured* (boot failure).
//! - `provider.rs` — the [`SocialProvider`](crate::SocialProvider) impl and its
//!   `inventory::submit!`, whose `build` is normally one call to
//!   [`resolve_provider`](crate::resolve_provider).
//!
//! There is no per-provider `module.rs`: a social provider is not a DI provider,
//! so it has nothing for a module of its own to own. Pinning config in code is
//! the ordinary config seam — provide the `XSocialConfig` value, which wins over
//! the environment.

/// First-party GitHub OAuth provider.
pub mod github;
/// First-party Google OIDC provider.
pub mod google;

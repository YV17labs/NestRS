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
//! so it has nothing for a module of its own to own. The config in `config.rs`
//! is a plain `#[config]` all the same, so it configures the way every other one
//! does — `NESTRS_SOCIAL__<KEY>__*`, over whatever base the deployment resolved
//! for that namespace. Nothing about it is declared on
//! [`SocialModule`](crate::SocialModule), which never learns the provider
//! exists.

/// First-party GitHub OAuth provider.
pub mod github;
/// First-party Google OIDC provider.
pub mod google;

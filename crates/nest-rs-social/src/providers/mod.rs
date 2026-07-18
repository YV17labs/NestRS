//! First-party providers. Each folder is the template a third-party provider
//! crate copies: a `config.rs` (dual-path env/pinned), a `module.rs`
//! (`DynamicModule` building the provider from config), and a `provider.rs`
//! (the trait impl + its `inventory::submit!`).

/// First-party GitHub OAuth provider.
pub mod github;
/// First-party Google OIDC provider.
pub mod google;

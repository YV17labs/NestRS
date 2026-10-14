//! First-party providers. Each folder is the template a third-party provider
//! crate copies. Three files are the whole contract:
//!
//! - `config.rs` — the dual-path `#[config]` type plus its
//!   [`SocialProviderConfig`](crate::SocialProviderConfig) impl, which decides
//!   *unconfigured* (inert) from *partially configured* (boot failure).
//! - `module.rs` — the `<Vendor>SocialModule` an app imports to open the
//!   provider; importing it is also what links the crate, so its entry exists.
//! - `provider.rs` — the [`SocialProvider`](crate::SocialProvider) impl and its
//!   `inventory::submit!`, whose `build` is normally one call to
//!   [`resolve_provider`](crate::resolve_provider) and whose `module` names
//!   the module above.

/// A required credential holding only whitespace is refused like an empty one.
pub(crate) fn not_blank(value: &str) -> Result<(), validator::ValidationError> {
    if value.trim().is_empty() {
        return Err(validator::ValidationError::new("blank"));
    }
    Ok(())
}

/// First-party GitHub OAuth provider.
pub mod github;
/// First-party Google OIDC provider.
pub mod google;

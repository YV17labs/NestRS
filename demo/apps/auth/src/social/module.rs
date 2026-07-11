use nest_rs_core::module;

use super::oauth_clients::SocialOAuthClientsModule;
use super::service::SocialLoginService;

/// Wires the keyed-provider exemplar: a service reaching two `OAuth2Client`s by
/// key. The keyed clients are registered by the imported
/// [`SocialOAuthClientsModule`]'s collect phase, so importing `SocialModule`
/// alone brings both — the service resolves them at boot without any root-level
/// seed (a missing client fails the boot naming the type and key).
#[module(
    imports = [SocialOAuthClientsModule::default()],
    providers = [SocialLoginService],
)]
pub struct SocialModule;

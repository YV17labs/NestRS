use std::sync::Arc;

use nest_rs_authn::{AuthError, Authorization, JwtService, OAuth2Client};
use nest_rs_core::injectable;

/// Exemplar for **keyed / multi-instance providers**: two OAuth2 provider
/// clients — GitHub and Google — live in the one flat container at once,
/// disambiguated by key instead of a per-provider newtype wrapper. The clients
/// are registered by `SocialModule` (see `oauth_clients.rs`) and reached here
/// through `#[inject(key = "…")]`. The access graph validates each keyed
/// dependency at boot exactly like a bare one — a missing keyed client fails the
/// boot naming both the type and the key.
#[injectable]
pub struct SocialLoginService {
    #[inject(key = "github")]
    github: Arc<OAuth2Client>,
    #[inject(key = "google")]
    google: Arc<OAuth2Client>,
    #[inject]
    jwt_svc: Arc<JwtService>,
}

impl SocialLoginService {
    /// Begin the Authorization-Code redirect for a named provider, or `None`
    /// when the provider is not one of the keyed clients.
    pub fn authorize(&self, provider: &str) -> Option<Result<Authorization, AuthError>> {
        let client = match provider {
            "github" => &self.github,
            "google" => &self.google,
            _ => return None,
        };
        Some(client.authorize(&self.jwt_svc))
    }
}

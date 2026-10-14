use nest_rs::config::{ClientTls, Config, ConfigService, config};
use nest_rs::oauth::client::OAuthClientConfig;
use nest_rs::social::SocialProviderConfig;

/// The probe's credentials, complete by default: a probe the app holds is never inert.
#[config(namespace = "social_probe")]
#[derive(Clone)]
pub struct ProbeSocialConfig {
    /// The OAuth client id.
    #[validate(length(min = 1))]
    pub client_id: String,
    /// The OAuth client secret.
    #[validate(length(min = 1))]
    pub client_secret: String,
}

impl Default for ProbeSocialConfig {
    fn default() -> Self {
        Self {
            client_id: "probe-client".into(),
            client_secret: "probe-secret".into(),
        }
    }
}

impl ProbeSocialConfig {
    pub(crate) fn oauth2_config(&self) -> OAuthClientConfig {
        OAuthClientConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            auth_url: "https://probe.example/authorize".into(),
            token_url: "https://probe.example/token".into(),
            redirect_url: "https://app.example/social/probe/callback".into(),
            userinfo_url: "https://probe.example/me".into(),
            scopes: Vec::new(),
            tls: ClientTls::default(),
        }
    }
}

impl Config for ProbeSocialConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs::config::Result<Self> {
        Ok(Self {
            client_id: env.get("CLIENT_ID")?.unwrap_or(base.client_id),
            client_secret: env.get("CLIENT_SECRET")?.unwrap_or(base.client_secret),
        })
    }
}

impl SocialProviderConfig for ProbeSocialConfig {
    fn is_unconfigured(&self) -> bool {
        self.client_id.is_empty() && self.client_secret.is_empty()
    }
}

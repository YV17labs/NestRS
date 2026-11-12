use nest_rs::config::{ClientTls, config};
use nest_rs::core::Secret;
use nest_rs::oauth::client::OAuthClientConfig;
use nest_rs::social::SocialProviderConfig;

/// The probe's credentials, complete by default: a probe the app holds is never inert.
#[config(namespace = "social_probe")]
#[derive(Clone, Debug)]
pub struct ProbeSocialConfig {
    /// The OAuth client id.
    #[validate(length(min = 1))]
    pub client_id: String,
    /// The OAuth client secret.
    #[validate(length(min = 1))]
    pub client_secret: Secret<String>,
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
            authorization_parameters: Default::default(),
            tls: ClientTls::default(),
        }
    }
}

impl SocialProviderConfig for ProbeSocialConfig {
    fn is_unconfigured(&self) -> bool {
        self.client_id.is_empty() && self.client_secret.expose_secret().is_empty()
    }
}

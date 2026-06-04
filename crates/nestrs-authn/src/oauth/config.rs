//! [`OAuth2Config`] — env-driven OAuth2 provider endpoints.

use nestrs_config::{Config, ConfigService, config};
use validator::Validate;

// No `Debug`: `client_secret` must not leak through a derived format.
#[config(namespace = "authn")]
#[derive(Clone, Default, Validate)]
pub struct OAuth2Config {
    #[validate(length(min = 1))]
    pub client_id: String,
    #[validate(length(min = 1))]
    pub client_secret: String,
    #[validate(length(min = 1))]
    pub auth_url: String,
    #[validate(length(min = 1))]
    pub token_url: String,
    #[validate(length(min = 1))]
    pub redirect_url: String,
    #[validate(length(min = 1))]
    pub userinfo_url: String,
    pub scopes: Vec<String>,
}

impl Config for OAuth2Config {
    fn from_env(env: &ConfigService) -> nestrs_config::Result<Self> {
        Ok(Self {
            client_id: env.get("CLIENT_ID").unwrap_or_default(),
            client_secret: env.get("CLIENT_SECRET").unwrap_or_default(),
            auth_url: env.get("AUTH_URL").unwrap_or_default(),
            token_url: env.get("TOKEN_URL").unwrap_or_default(),
            redirect_url: env.get("REDIRECT_URL").unwrap_or_default(),
            userinfo_url: env.get("USERINFO_URL").unwrap_or_default(),
            scopes: env.list("SCOPES"),
        })
    }
}

//! [`OAuth2Config`] — env-driven OAuth2 provider endpoints.

use nest_rs_config::{Config, ConfigService, config};
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
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> OAuth2Config {
        OAuth2Config {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://auth.example/oauth/authorize".into(),
            token_url: "https://auth.example/oauth/token".into(),
            redirect_url: "https://app.example/oauth/callback".into(),
            userinfo_url: "https://auth.example/oauth/userinfo".into(),
            scopes: vec!["read".into()],
        }
    }

    #[test]
    fn default_is_invalid_until_every_url_is_filled() {
        let err = OAuth2Config::default().validate().unwrap_err();
        let fields = err.field_errors();
        // Every length-1 validation must trip — including client_secret which
        // means an unconfigured app fails the boot loudly rather than
        // accepting the empty default.
        for required in [
            "client_id",
            "client_secret",
            "auth_url",
            "token_url",
            "redirect_url",
            "userinfo_url",
        ] {
            assert!(
                fields.contains_key(required),
                "expected {required} in {:?}",
                fields.keys().collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn a_complete_config_validates() {
        complete().validate().expect("valid");
    }

    #[test]
    fn each_required_field_is_individually_load_bearing() {
        // Each of the six URL/credential fields blocks validation on its own —
        // a regression that allowed any of them to default to "" would let an
        // app boot with broken OAuth flow.
        type Setter = fn(&mut OAuth2Config);
        let setters: [(Setter, &str); 6] = [
            (|c| c.client_id = String::new(), "client_id"),
            (|c| c.client_secret = String::new(), "client_secret"),
            (|c| c.auth_url = String::new(), "auth_url"),
            (|c| c.token_url = String::new(), "token_url"),
            (|c| c.redirect_url = String::new(), "redirect_url"),
            (|c| c.userinfo_url = String::new(), "userinfo_url"),
        ];
        for (mutate, field) in setters {
            let mut cfg = complete();
            mutate(&mut cfg);
            let err = cfg.validate().unwrap_err();
            assert!(
                err.field_errors().contains_key(field),
                "blanking {field} did not trip validation: {err:?}",
            );
        }
    }

    #[test]
    fn scopes_default_to_empty_and_take_an_explicit_list() {
        assert!(OAuth2Config::default().scopes.is_empty());
        let cfg = OAuth2Config {
            scopes: vec!["read".into(), "write".into()],
            ..complete()
        };
        assert_eq!(cfg.scopes, vec!["read".to_string(), "write".into()]);
    }
}

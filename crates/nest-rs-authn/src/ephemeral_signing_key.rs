//! [`EphemeralSigningKey`] — the key pair a test holds, which the app's
//! [`JwtService`](crate::JwtService) verifies with in place of the key material
//! its configuration names.

use nest_rs_core::Secret;

use crate::{Algorithm, AuthError, AuthnConfig, JwtKey, JwtOptions};

/// An Ed25519 pair the test harness issued in process and seeded, so a suite
/// signs the tokens its app verifies without a committed key: the app's
/// `JwtService` verifies with the public half in place of its configured key
/// material, and signs with the private half only when that material signs — a
/// verify-only app stays one, and a configuration naming no key, the
/// deployment's to set, verifies only. Every other setting stays the app's: the
/// audience, the issuer, the resource it serves.
pub struct EphemeralSigningKey {
    /// The private half, PKCS #8 PEM.
    pub private_pem: Secret<String>,
    /// The public half, SPKI PEM.
    pub public_pem: String,
}

impl EphemeralSigningKey {
    /// `config`'s options over this pair in place of their key material,
    /// signing only where theirs signs, whatever else `config` refuses still
    /// refused.
    pub(crate) fn options_from(&self, mut config: AuthnConfig) -> Result<JwtOptions, AuthError> {
        let names_a_key = config.secret.is_some()
            || config.private_key.is_some()
            || config.public_key.is_some()
            || config.jwks_uri.is_some();
        if !names_a_key {
            config.public_key = Some(self.public_pem.clone());
        }
        let mut options = config.into_options()?;
        let signs = match &options.key {
            JwtKey::Hmac(_) => true,
            JwtKey::Pem { private_pem, .. } => private_pem.is_some(),
            JwtKey::Jwks { .. } => false,
        };
        options.key = JwtKey::Pem {
            private_pem: signs.then(|| self.private_pem.clone()),
            public_pem: self.public_pem.clone(),
        };
        options.algorithms = vec![Algorithm::EdDSA];
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> EphemeralSigningKey {
        EphemeralSigningKey {
            private_pem: Secret::new("private".into()),
            public_pem: "public".into(),
        }
    }

    fn secret() -> Option<Secret<String>> {
        Some(Secret::new("a-secret".into()))
    }

    #[test]
    fn the_pair_signs_only_where_the_configured_key_signs() {
        for (configured, signs) in [
            (
                AuthnConfig {
                    secret: secret(),
                    ..AuthnConfig::default()
                },
                true,
            ),
            (
                AuthnConfig {
                    private_key: Some(Secret::new("configured private".into())),
                    public_key: Some("configured public".into()),
                    ..AuthnConfig::default()
                },
                true,
            ),
            (
                AuthnConfig {
                    public_key: Some("configured public".into()),
                    ..AuthnConfig::default()
                },
                false,
            ),
            (
                AuthnConfig {
                    jwks_uri: Some("https://issuer.example/jwks.json".into()),
                    ..AuthnConfig::default()
                },
                false,
            ),
            (AuthnConfig::default(), false),
        ] {
            let described = format!("{configured:?}");
            let replaced = pair()
                .options_from(configured)
                .expect("the configuration is one the boot accepts");
            let JwtKey::Pem {
                private_pem,
                public_pem,
            } = replaced.key
            else {
                panic!("{described} is replaced by a PEM pair");
            };
            assert_eq!(public_pem, "public", "{described}");
            assert_eq!(private_pem.is_some(), signs, "{described}");
            assert_eq!(replaced.algorithms, [Algorithm::EdDSA], "{described}");
        }
    }

    #[test]
    fn every_other_setting_stays_the_apps() {
        let replaced = pair()
            .options_from(AuthnConfig {
                public_key: Some("configured public".into()),
                audience: Some("https://api.example.com".into()),
                issuer: Some("https://auth.example.com".into()),
                explicit_typing: false,
                expires_in: std::time::Duration::from_secs(60),
                ..AuthnConfig::default()
            })
            .expect("a public key is a configuration the boot accepts");

        assert_eq!(
            replaced.audience.as_deref(),
            Some("https://api.example.com")
        );
        assert_eq!(replaced.issuer.as_deref(), Some("https://auth.example.com"));
        assert!(!replaced.explicit_typing);
        assert_eq!(replaced.expires_in, std::time::Duration::from_secs(60));
    }

    #[test]
    fn what_the_configuration_refuses_it_still_refuses() {
        let authorities = nest_rs_config::Material::inline("an authority");
        for configured in [
            AuthnConfig {
                private_key: Some(Secret::new("configured private".into())),
                ..AuthnConfig::default()
            },
            AuthnConfig {
                secret: secret(),
                public_key: Some("configured public".into()),
                ..AuthnConfig::default()
            },
            AuthnConfig {
                secret: secret(),
                jwks_uri: Some("https://issuer.example/jwks.json".into()),
                ..AuthnConfig::default()
            },
            AuthnConfig {
                tls: nest_rs_config::ClientTls::new(Some(authorities), None),
                ..AuthnConfig::default()
            },
        ] {
            let said = configured
                .clone()
                .into_options()
                .expect_err("the boot refuses this configuration")
                .to_string();
            let refused = pair()
                .options_from(configured)
                .expect_err("a key of the test's accepts nothing the boot refuses");
            assert_eq!(refused.to_string(), said);
        }
    }
}

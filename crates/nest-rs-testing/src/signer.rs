//! The tokens a test signs for the app it booted (feature `authn`): a key pair
//! of the test's own, issued in process as [`TestAuthority`](crate::TestAuthority)
//! issues certificates, which the app verifies with in place of its configured
//! key material, so no app's suite commits a private key.

use std::time::Duration;

use anyhow::{Context as _, Result};
use nest_rs_authn::__private::{EphemeralSigningKey, ResourceBinding, sign_unchecked};
use nest_rs_authn::{AuthnConfig, JwtOptions, JwtService};
use nest_rs_core::{Container, Secret};
use serde::Serialize;
use serde_json::Value;

use crate::TestAppBuilder;
use crate::app::Kept;

impl TestAppBuilder {
    /// Boot verifying the tokens of a key pair of the test's own, issued for
    /// this app alone: its `JwtService` verifies with the public half in place
    /// of the key material its configuration names — a secret, a key, an
    /// issuer's JWK Set — and signs with the private half only where that
    /// material signs, so a verify-only app stays one, and a configuration
    /// naming no key verifies only. Its audience, issuer and the resource it
    /// serves stay as it resolves them, and `bearer` signs a token they accept.
    ///
    /// The boot refuses an app that provides no `JwtService`, and one whose
    /// `JwtService` refuses a token of the pair — seeded or overridden past the
    /// module building it.
    pub fn ephemeral_signing_key(self) -> Self {
        let pair =
            rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("an Ed25519 key generates");
        self.provide(EphemeralSigningKey {
            private_pem: Secret::new(pair.serialize_pem()),
            public_pem: pair.public_key_pem(),
        })
    }
}

/// The private half of an app's ephemeral signing key, signing for the
/// audience and as the issuer the app requires.
pub(crate) struct Signer {
    jwt: JwtService,
}

impl Signer {
    /// The signer of the tokens the booted app in `container` verifies with its
    /// ephemeral key, proven on one it signs; `None` without a key.
    pub(crate) async fn proven(container: &Container) -> Result<Option<Self>> {
        let Some(key) = container.get::<EphemeralSigningKey>() else {
            return Ok(None);
        };
        let verifier = container.get::<JwtService>().context(
            "ephemeral_signing_key installs the key an app verifies tokens with, and this app \
             provides no JwtService: import AuthnModule, or the module authenticating the \
             routes the test calls",
        )?;
        let mut options = JwtOptions::eddsa(
            key.private_pem.expose_secret().clone(),
            key.public_pem.clone(),
        );
        options.expires_in = Duration::from_secs(verifier.ttl_secs());
        if let Some(binding) = container.get::<ResourceBinding>() {
            options.audience = Some(binding.audience.clone());
            options.issuer = binding.issuers.first().cloned();
        } else if let Some(config) = container.get::<AuthnConfig>() {
            options.audience = config.audience.clone();
            options.issuer = config.issuer.clone();
        }
        let signer = Self {
            jwt: JwtService::new(options)?,
        };
        let probe = sign_unchecked(
            &signer.jwt,
            &serde_json::json!({ "sub": "nest-rs-testing" }),
        )?;
        verifier.verify::<Value>(&probe).await.context(
            "the app's JwtService refuses a token of its ephemeral signing key: it was seeded or \
             overridden past the module that builds it from AuthnConfig",
        )?;
        Ok(Some(signer))
    }
}

impl Kept {
    /// The `Authorization` value of `claims` signed by the app's ephemeral key,
    /// as they are written: a test forges what the app must refuse.
    pub(crate) fn bearer(&self, claims: &impl Serialize) -> String {
        let signer = self.signer.as_ref().expect(
            "the app was booted with no signing key of the test's own: add \
             `.ephemeral_signing_key()` to its builder",
        );
        let token = sign_unchecked(&signer.jwt, claims)
            .unwrap_or_else(|refused| panic!("the claims could not be signed: {refused}"));
        format!("Bearer {token}")
    }
}

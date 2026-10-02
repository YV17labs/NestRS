//! [`AudienceBinding`] — the confused-deputy check [`OAuthResourceModule`]
//! makes mandatory, run once wiring is complete.
//!
//! [`OAuthResourceModule`]: crate::OAuthResourceModule

use std::sync::Arc;

use nest_rs_authn::AuthnConfig;
use nest_rs_config::Namespaced;
use nest_rs_core::{hooks, injectable};

use crate::metadata::ProtectedResourceMetadata;

/// Runs the confused-deputy check once wiring is complete.
///
/// It is a lifecycle hook rather than part of the metadata factory because
/// [`AuthnConfig`] is itself a factory output: reading it *during* the factory
/// phase depends on which module was collected first, which is exactly the kind
/// of ordering a boot check must not rest on. `#[on_module_init]` runs after
/// every provider is built, so the answer is the same whatever the import order
/// — and an `Err` there aborts boot.
#[injectable]
pub(crate) struct AudienceBinding {
    /// Required, not `Option`: `OAuthResourceHost` is private and
    /// `OAuthResourceSetup` always queues the factory, so the absent case
    /// was a branch nothing could reach — and one that would have passed the
    /// confused-deputy check silently if anything ever did.
    #[inject]
    metadata: Arc<ProtectedResourceMetadata>,
    #[inject]
    authn: Option<Arc<AuthnConfig>>,
}

#[hooks]
impl AudienceBinding {
    #[on_module_init]
    async fn verify(&self) -> anyhow::Result<()> {
        require_audience_binding(self.authn.as_deref(), &self.metadata)
    }
}

/// The confused-deputy check, run once at boot.
///
/// A resource server that does not pin `aud` accepts any validly-signed token
/// from its issuer — including one a user granted to a different service, which
/// that service can then replay here. RFC 8707 makes the resource identifier
/// *be* the audience, so the two must agree; a mismatch is a `warn` rather than
/// a refusal because some authorization servers mint an opaque audience string
/// by policy, and the deployment is entitled to that.
fn require_audience_binding(
    authn: Option<&AuthnConfig>,
    metadata: &ProtectedResourceMetadata,
) -> anyhow::Result<()> {
    let Some(authn) = authn else {
        anyhow::bail!(
            "OAuthResourceModule needs the token verifier it protects: import \
             AuthnModule::for_root(..) alongside it"
        );
    };
    let audience = authn.audience.as_deref().map(str::trim).unwrap_or_default();
    if audience.is_empty() {
        anyhow::bail!(
            "{} is required when OAuthResourceModule is imported: \
             without it this server accepts any token its issuer signed, including one minted \
             for another service. Set it to `{}`",
            nest_rs_config::var_name(AuthnConfig::NAMESPACE, "AUDIENCE"),
            metadata.resource(),
        );
    }
    if audience != metadata.resource() {
        tracing::warn!(
            target: crate::TARGET,
            audience,
            resource = metadata.resource(),
            "token audience differs from the advertised resource identifier — a client \
             following RFC 8707 will request `resource` and receive a token this server rejects",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OAuthResourceConfig;

    fn metadata() -> ProtectedResourceMetadata {
        OAuthResourceConfig::default()
            .with_resource("https://api.example.com")
            .with_authorization_servers(["https://auth.example.com"])
            .into_metadata()
            .expect("valid config")
    }

    fn authn_with_audience(audience: Option<&str>) -> AuthnConfig {
        AuthnConfig {
            audience: audience.map(str::to_owned),
            ..AuthnConfig::default()
        }
    }

    #[test]
    fn a_matching_audience_passes() {
        require_audience_binding(
            Some(&authn_with_audience(Some("https://api.example.com"))),
            &metadata(),
        )
        .expect("audience matches the resource");
    }

    /// A mismatch is a `warn`, not a refusal — some authorization servers mint
    /// an opaque audience by policy and the deployment is entitled to that.
    ///
    /// Which is exactly why the line matters: the app boots and serves, and the
    /// failure surfaces one client at a time, as a `401` on a token that
    /// followed RFC 8707 correctly. Nothing else in the system knows the two
    /// values disagree — this is the only place both are in scope.
    #[test]
    fn a_mismatched_audience_boots_and_says_which_two_values_disagree() {
        let logs = nest_rs_testing::LogCapture::install();
        require_audience_binding(
            Some(&authn_with_audience(Some("some-opaque-audience"))),
            &metadata(),
        )
        .expect("a mismatch is tolerated, not refused");

        let event = logs.expect_one(
            crate::TARGET,
            "token audience differs from the advertised resource identifier — a client \
             following RFC 8707 will request `resource` and receive a token this server rejects",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("audience").as_deref(),
            Some("some-opaque-audience")
        );
        assert_eq!(
            event.field("resource").as_deref(),
            Some("https://api.example.com"),
        );
    }

    /// And a matching pair is silent: warning on the correct configuration is
    /// how a reader learns to ignore the line above.
    #[test]
    fn a_matching_audience_says_nothing() {
        let logs = nest_rs_testing::LogCapture::install();
        require_audience_binding(
            Some(&authn_with_audience(Some("https://api.example.com"))),
            &metadata(),
        )
        .expect("audience matches the resource");
        assert!(
            logs.events().is_empty(),
            "the correct configuration is not an event: {:#?}",
            logs.events(),
        );
    }

    #[test]
    fn a_missing_audience_fails_boot_and_names_the_variable() {
        let err = require_audience_binding(Some(&authn_with_audience(None)), &metadata())
            .expect_err("no audience");
        let text = err.to_string();
        assert!(
            text.contains(&nest_rs_config::var_name(
                AuthnConfig::NAMESPACE,
                "AUDIENCE"
            )),
            "got: {text}",
        );
        assert!(
            text.contains("https://api.example.com"),
            "the error must say what to set it to: {text}",
        );
    }

    #[test]
    fn a_blank_audience_counts_as_missing() {
        // `NESTRS_AUTHN__AUDIENCE=` in a `.env` reads as `Some("")` — a value
        // that would disable the check while looking configured.
        assert!(
            require_audience_binding(Some(&authn_with_audience(Some("  "))), &metadata()).is_err(),
        );
    }

    #[test]
    fn no_authn_config_at_all_fails_boot_naming_the_missing_module() {
        let err = require_audience_binding(None, &metadata()).expect_err("no verifier");
        assert!(
            err.to_string().contains("AuthnModule::for_root"),
            "got: {err}"
        );
    }

    #[test]
    fn a_differing_audience_is_allowed_but_warned() {
        // Not every authorization server mints the resource URI as `aud`; the
        // deployment keeps the choice, the log keeps the record.
        require_audience_binding(Some(&authn_with_audience(Some("api"))), &metadata())
            .expect("a differing audience is a warn, not a refusal");
    }
}

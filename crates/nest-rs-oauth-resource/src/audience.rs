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
/// A hook, not part of the metadata factory: [`AuthnConfig`] is a factory output,
/// and reading it there would depend on the order modules are collected in.
#[injectable]
pub(crate) struct AudienceBinding {
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
/// A mismatch with the resource (RFC 8707) only warns: some authorization servers
/// mint an opaque audience by policy.
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

    /// A mismatch is a `warn`, not a refusal: some authorization servers mint an
    /// opaque audience by policy.
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
        // `<PREFIX>_AUTHN__AUDIENCE=` in a `.env` reads as `Some("")` — a value
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
        require_audience_binding(Some(&authn_with_audience(Some("api"))), &metadata())
            .expect("a differing audience is a warn, not a refusal");
    }
}

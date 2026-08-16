//! [`ProtectedResourceModule`] — turns this app into a conformant OAuth 2.1
//! resource server.
//!
//! Importing it does three things, all of them at boot so a misconfiguration is
//! a build-time failure rather than a `401` nobody can act on:
//!
//! 1. Validates [`ProtectedResourceConfig`] into the served
//!    [`ProtectedResourceMetadata`], and provides it as global infrastructure.
//! 2. Mounts `GET /.well-known/oauth-protected-resource` (RFC 9728 §3),
//!    declared `#[public]`.
//! 3. Attaches [`ResourceChallenge`](super::interceptor::ResourceChallenge), so
//!    every `401` carries `resource_metadata`.
//!
//! **And it makes audience validation mandatory.** The MCP authorization spec
//! requires a server to verify that a token was issued *for it* — the defence
//! against a confused deputy replaying a token minted for another service.
//! `NESTRS_AUTHN__AUDIENCE` is optional in [`JwtConfig`] on its own; under this
//! module it is required, and boot fails naming it. That is the whole point of
//! the capability: without it the well-known document advertises a resource
//! identity the verifier never checks.

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, Module, hooks, injectable, module};

use crate::jwt::JwtConfig;
use crate::resource::config::ProtectedResourceConfig;
use crate::resource::controller::ProtectedResourceController;
use crate::resource::interceptor::ResourceChallenge;
use crate::resource::metadata::ProtectedResourceMetadata;

/// The discovery surface itself. Private, and deliberately: it is inert without
/// the [`ProtectedResourceMetadata`] factory that only
/// [`ProtectedResourceSetup`] queues, so a bare `imports = [..]` of it would
/// fail boot on an unmet dependency. `for_root` is the single seam, the same
/// shape [`AuthnModule`](crate::AuthnModule) has.
#[module(
    imports = [ConfigModule::for_feature::<ProtectedResourceConfig>()],
    providers = [ProtectedResourceController, ResourceChallenge, AudienceBinding],
)]
struct ProtectedResourceHost;

/// Runs the confused-deputy check once wiring is complete.
///
/// It is a lifecycle hook rather than part of the metadata factory because
/// [`JwtConfig`] is itself a factory output: reading it *during* the factory
/// phase depends on which module was collected first, which is exactly the kind
/// of ordering a boot check must not rest on. `#[on_module_init]` runs after
/// every provider is built, so the answer is the same whatever the import order
/// — and an `Err` there aborts boot.
#[injectable]
struct AudienceBinding {
    /// Required, not `Option`: `ProtectedResourceHost` is private and
    /// `ProtectedResourceSetup` always queues the factory, so the absent case
    /// was a branch nothing could reach — and one that would have passed the
    /// confused-deputy check silently if anything ever did.
    #[inject]
    metadata: Arc<ProtectedResourceMetadata>,
    #[inject]
    jwt: Option<Arc<JwtConfig>>,
}

#[hooks]
impl AudienceBinding {
    #[on_module_init]
    async fn verify(&self) -> anyhow::Result<()> {
        require_audience_binding(self.jwt.as_deref(), &self.metadata)
    }
}

/// DI module for the RFC 9728 discovery surface. See the module docs for what
/// importing it enforces.
pub struct ProtectedResourceModule;

impl ProtectedResourceModule {
    /// `None` ⇒ load [`ProtectedResourceConfig`] from `NESTRS_AUTHN__*`;
    /// `Some(cfg)` makes `cfg` the base those variables overlay per field.
    pub fn for_root(config: impl Into<Option<ProtectedResourceConfig>>) -> ProtectedResourceSetup {
        ProtectedResourceSetup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`ProtectedResourceModule::for_root`].
pub struct ProtectedResourceSetup {
    pinned: Option<ProtectedResourceConfig>,
}

impl DynamicModule for ProtectedResourceSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<ProtectedResourceMetadata, _, _>(|container| async move {
            let config = container
                .get::<ProtectedResourceConfig>()
                .expect("ProtectedResourceConfig is resolved by ConfigModule::provide_feature");
            let metadata = (*config)
                .clone()
                .into_metadata()
                .map_err(anyhow::Error::new)?;
            tracing::debug!(
                target: "nest_rs::authn",
                resource = metadata.resource(),
                authorization_servers = metadata.authorization_servers().len(),
                "protected resource metadata resolved",
            );
            Ok(metadata)
        })
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        <ProtectedResourceHost as Module>::register(builder)
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
    jwt: Option<&JwtConfig>,
    metadata: &ProtectedResourceMetadata,
) -> anyhow::Result<()> {
    let Some(jwt) = jwt else {
        anyhow::bail!(
            "ProtectedResourceModule needs the token verifier it protects: import \
             AuthnModule::for_root(..) alongside it"
        );
    };
    let audience = jwt.audience.as_deref().map(str::trim).unwrap_or_default();
    if audience.is_empty() {
        anyhow::bail!(
            "{} is required when ProtectedResourceModule is imported: \
             without it this server accepts any token its issuer signed, including one minted \
             for another service. Set it to `{}`",
            nest_rs_config::var_name("authn", "AUDIENCE"),
            metadata.resource(),
        );
    }
    if audience != metadata.resource() {
        tracing::warn!(
            target: "nest_rs::authn",
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

    fn metadata() -> ProtectedResourceMetadata {
        ProtectedResourceConfig::default()
            .with_resource("https://api.example.com")
            .with_authorization_servers(["https://auth.example.com"])
            .into_metadata()
            .expect("valid config")
    }

    fn jwt_with_audience(audience: Option<&str>) -> JwtConfig {
        JwtConfig {
            audience: audience.map(str::to_owned),
            ..JwtConfig::default()
        }
    }

    #[test]
    fn a_matching_audience_passes() {
        require_audience_binding(
            Some(&jwt_with_audience(Some("https://api.example.com"))),
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
            Some(&jwt_with_audience(Some("some-opaque-audience"))),
            &metadata(),
        )
        .expect("a mismatch is tolerated, not refused");

        let event = logs.expect_one(
            "nest_rs::authn",
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
            Some(&jwt_with_audience(Some("https://api.example.com"))),
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
        let err = require_audience_binding(Some(&jwt_with_audience(None)), &metadata())
            .expect_err("no audience");
        let text = err.to_string();
        assert!(text.contains("NESTRS_AUTHN__AUDIENCE"), "got: {text}");
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
            require_audience_binding(Some(&jwt_with_audience(Some("  "))), &metadata()).is_err(),
        );
    }

    #[test]
    fn no_jwt_config_at_all_fails_boot_naming_the_missing_module() {
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
        require_audience_binding(Some(&jwt_with_audience(Some("api"))), &metadata())
            .expect("a differing audience is a warn, not a refusal");
    }
}

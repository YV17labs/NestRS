//! [`OAuthResourceModule`] — turns this app into a conformant OAuth 2.1
//! resource server.
//!
//! Imported bare — or through `for_root`, to pin a base its variables
//! overlay — it, at boot:
//!
//! 1. Validates [`OAuthResourceConfig`] into the served
//!    [`ProtectedResourceMetadata`], and provides it as global infrastructure.
//! 2. Mounts `GET /.well-known/oauth-protected-resource` (RFC 9728 §3),
//!    declared `#[public]`.
//! 3. Attaches [`ResourceChallenge`], so every `401` carries `resource_metadata`.
//!
//! The endpoint and the challenge ride the HTTP transport it imports bare.
//!
//! It imports the token verifier, [`AuthnModule`] bare, and binds it to this
//! resource — the MCP authorization spec's confused-deputy defence: a token is
//! accepted only when its `aud` is the resource (RFC 8707;
//! `<PREFIX>_AUTHN__AUDIENCE` when set) and its `iss` one of the authorization
//! servers (RFC 9068 §4). A resource server with no key to verify with fails
//! the boot as the verifier does, naming the variables that give it one; one
//! whose verifier is configured to contradict the binding fails naming the
//! settings. And the MCP endpoint's `Host` allowlist admits the host the
//! resource names.

use nest_rs_authn::__private::ResourceBinding;
use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_config::{ConfigModule, ConfigSetup, endpoint_shown};
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering, module};
use nest_rs_http::__private::CanonicalHost;
use nest_rs_http::HttpModule;

use crate::config::OAuthResourceConfig;
use crate::controller::OAuthResourceController;
use crate::interceptor::ResourceChallenge;
use crate::metadata::ProtectedResourceMetadata;

/// DI module for the RFC 9728 discovery surface. Imported bare, it reads
/// `<PREFIX>_OAUTH__RESOURCE__*`; [`OAuthResourceModule::for_root`] pins a base
/// those variables overlay.
#[module(
    imports = [
        nest_rs_config::__private::feature::<OAuthResourceConfig>(),
        AuthnModule,
        HttpModule,
        OAuthResourceWiring,
    ],
    providers = [OAuthResourceController, ResourceChallenge],
)]
pub struct OAuthResourceModule;

impl OAuthResourceModule {
    /// Pin `config` as the base `<PREFIX>_OAUTH__RESOURCE__*` overlays, per
    /// field; the bare import reads those variables over
    /// [`OAuthResourceConfig`]'s defaults.
    pub fn for_root(config: OAuthResourceConfig) -> OAuthResourceSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import [`OAuthResourceModule::for_root`] returns.
pub type OAuthResourceSetup = ConfigSetup<OAuthResourceModule, OAuthResourceConfig>;

/// Queues the served document and what it binds: `#[module]` takes only
/// imports and providers, so this hand-written module carries the factories
/// for [`OAuthResourceModule`].
struct OAuthResourceWiring;

impl Module for OAuthResourceWiring {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_factory_after::<ProtectedResourceMetadata, (OAuthResourceConfig,), _, _>(
                |container| async move {
                    let config = ConfigModule::resolved::<OAuthResourceConfig>(&container)?;
                    frozen((*config).clone())
                },
            )
            .provide_factory_after::<ResourceBinding, (AuthnConfig, ProtectedResourceMetadata), _, _>(
                |container| async move {
                    let metadata = container.get::<ProtectedResourceMetadata>().ok_or_else(|| {
                        anyhow::anyhow!("the protected resource metadata did not resolve")
                    })?;
                    let authn = container.get::<AuthnConfig>().ok_or_else(|| {
                        anyhow::anyhow!("the token verifier's configuration did not resolve")
                    })?;
                    crate::binding::bind(&authn, &metadata)
                },
            )
            .provide_factory_after::<CanonicalHost, (ProtectedResourceMetadata,), _, _>(
                |container| async move {
                    container
                        .get::<ProtectedResourceMetadata>()
                        .and_then(|metadata| CanonicalHost::of(metadata.resource()))
                        .ok_or_else(|| anyhow::anyhow!("the protected resource names no host"))
                },
            )
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

/// The document `config` describes, said once at `debug` with the resource as
/// [`endpoint_shown`] shows it.
fn frozen(config: OAuthResourceConfig) -> anyhow::Result<ProtectedResourceMetadata> {
    let metadata = config.into_metadata().map_err(anyhow::Error::new)?;
    tracing::debug!(
        target: crate::TARGET,
        resource = endpoint_shown(metadata.resource()).as_str(),
        authorization_servers = metadata.authorization_servers().len(),
        "protected resource metadata resolved",
    );
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resource is shown as `endpoint_shown` shows it: its query never.
    #[test]
    fn the_resolved_document_is_said_without_the_resource_query() {
        let logs = nest_rs_testing::LogCapture::install();
        frozen(OAuthResourceConfig {
            resource: Some("https://api.example.com/tenants?key=s3cr3t".into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..Default::default()
        })
        .expect("a valid identity");

        let event = logs.expect_one(crate::TARGET, "protected resource metadata resolved");
        assert_eq!(
            event.field("resource").as_deref(),
            Some("https://api.example.com/tenants")
        );
        assert!(
            !format!("{:?}", logs.events()).contains("s3cr3t"),
            "{:?}",
            logs.events()
        );
    }
}

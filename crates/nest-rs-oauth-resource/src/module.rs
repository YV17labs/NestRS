//! [`OAuthResourceModule`] — turns this app into a conformant OAuth 2.1
//! resource server.
//!
//! Importing it, at boot:
//!
//! 1. Validates [`OAuthResourceConfig`] into the served
//!    [`ProtectedResourceMetadata`], and provides it as global infrastructure.
//! 2. Mounts `GET /.well-known/oauth-protected-resource` (RFC 9728 §3),
//!    declared `#[public]`.
//! 3. Attaches [`ResourceChallenge`], so every `401` carries `resource_metadata`.
//!
//! The endpoint and the challenge ride the HTTP transport it imports bare.
//!
//! It also binds the token verifier [`AuthnModule`](nest_rs_authn::AuthnModule)
//! builds to this resource — the MCP authorization spec's confused-deputy
//! defence: a token is accepted only when its `aud` is the resource (RFC 8707;
//! `<PREFIX>_AUTHN__AUDIENCE` when set) and its `iss` one of the authorization
//! servers (RFC 9068 §4). The boot fails when the verifier is missing, or when
//! what it is configured with contradicts the binding, naming the settings.
//! And the MCP endpoint's `Host` allowlist admits the host the resource names.

use std::any::TypeId;

use nest_rs_authn::__private::ResourceBinding;
use nest_rs_authn::AuthnConfig;
use nest_rs_config::{ConfigModule, endpoint_shown};
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Registering, module};
use nest_rs_http::__private::CanonicalHost;
use nest_rs_http::HttpModule;

use crate::config::OAuthResourceConfig;
use crate::controller::OAuthResourceController;
use crate::interceptor::ResourceChallenge;
use crate::metadata::ProtectedResourceMetadata;

/// The discovery surface, private: inert without the [`ProtectedResourceMetadata`]
/// factory only [`OAuthResourceSetup`] queues, so `for_root` is the single seam.
#[module(
    imports = [nest_rs_config::__private::feature::<OAuthResourceConfig>(), HttpModule],
    providers = [OAuthResourceController, ResourceChallenge],
)]
struct OAuthResourceHost;

/// DI module for the RFC 9728 discovery surface.
pub struct OAuthResourceModule;

impl OAuthResourceModule {
    /// `None` ⇒ load [`OAuthResourceConfig`] from `<PREFIX>_OAUTH__RESOURCE__*`;
    /// `Some(cfg)` makes `cfg` the base those variables overlay per field.
    pub fn for_root(config: impl Into<Option<OAuthResourceConfig>>) -> OAuthResourceSetup {
        OAuthResourceSetup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`OAuthResourceModule::for_root`].
pub struct OAuthResourceSetup {
    pinned: Option<OAuthResourceConfig>,
}

impl DynamicModule for OAuthResourceSetup {
    fn module() -> TypeId {
        TypeId::of::<OAuthResourceHost>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(
            self.pinned.clone(),
            builder.import::<OAuthResourceHost>(),
        );
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
                    crate::binding::bind(container.get::<AuthnConfig>().as_deref(), &metadata)
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

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<OAuthResourceHost>()
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

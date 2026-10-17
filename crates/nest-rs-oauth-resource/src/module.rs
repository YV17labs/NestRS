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
//! It also makes `<PREFIX>_AUTHN__AUDIENCE`, optional in
//! [`AuthnConfig`](nest_rs_authn::AuthnConfig), required — the MCP authorization
//! spec's confused-deputy defence — and boot fails naming it.

use std::any::TypeId;

use nest_rs_config::{ConfigModule, endpoint_shown};
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Registering, module};

use crate::audience::AudienceBinding;
use crate::config::OAuthResourceConfig;
use crate::controller::OAuthResourceController;
use crate::interceptor::ResourceChallenge;
use crate::metadata::ProtectedResourceMetadata;

/// The discovery surface, private: inert without the [`ProtectedResourceMetadata`]
/// factory only [`OAuthResourceSetup`] queues, so `for_root` is the single seam.
#[module(
    imports = [ConfigModule::for_feature::<OAuthResourceConfig>()],
    providers = [OAuthResourceController, ResourceChallenge, AudienceBinding],
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
        builder.provide_factory_after::<ProtectedResourceMetadata, (OAuthResourceConfig,), _, _>(
            |container| async move {
                let config = ConfigModule::resolved::<OAuthResourceConfig>(&container)?;
                frozen((*config).clone())
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

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

use nest_rs_config::ConfigModule;
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
        builder.provide_factory::<ProtectedResourceMetadata, _, _>(|container| async move {
            #[expect(
                clippy::expect_used,
                reason = "provide_feature queued the config's factory in this module's collect"
            )]
            let config = container
                .get::<OAuthResourceConfig>()
                .expect("OAuthResourceConfig is resolved by ConfigModule::provide_feature");
            let metadata = (*config)
                .clone()
                .into_metadata()
                .map_err(anyhow::Error::new)?;
            tracing::debug!(
                target: crate::TARGET,
                resource = metadata.resource(),
                authorization_servers = metadata.authorization_servers().len(),
                "protected resource metadata resolved",
            );
            Ok(metadata)
        })
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<OAuthResourceHost>()
    }
}

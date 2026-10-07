//! [`OAuthResourceModule`] — turns this app into a conformant OAuth 2.1
//! resource server.
//!
//! Importing it does three things, all of them at boot so a misconfiguration is
//! a build-time failure rather than a `401` nobody can act on:
//!
//! 1. Validates [`OAuthResourceConfig`] into the served
//!    [`ProtectedResourceMetadata`], and provides it as global infrastructure.
//! 2. Mounts `GET /.well-known/oauth-protected-resource` (RFC 9728 §3),
//!    declared `#[public]`.
//! 3. Attaches [`ResourceChallenge`], so
//!    every `401` carries `resource_metadata`.
//!
//! **And it makes audience validation mandatory.** The MCP authorization spec
//! requires a server to verify that a token was issued *for it* — the defence
//! against a confused deputy replaying a token minted for another service.
//! `<PREFIX>_AUTHN__AUDIENCE` is optional in [`AuthnConfig`](nest_rs_authn::AuthnConfig) on its own; under this
//! module it is required, and boot fails naming it. That is the whole point of
//! the capability: without it the well-known document advertises a resource
//! identity the verifier never checks.

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, Imported, module};

use crate::audience::AudienceBinding;
use crate::config::OAuthResourceConfig;
use crate::controller::OAuthResourceController;
use crate::interceptor::ResourceChallenge;
use crate::metadata::ProtectedResourceMetadata;

/// The discovery surface itself. Private, and deliberately: it is inert without
/// the [`ProtectedResourceMetadata`] factory that only
/// [`OAuthResourceSetup`] queues, so a bare `imports = [..]` of it would
/// fail boot on an unmet dependency. `for_root` is the single seam, the same
/// shape [`AuthnModule`](nest_rs_authn::AuthnModule) has.
#[module(
    imports = [ConfigModule::for_feature::<OAuthResourceConfig>()],
    providers = [OAuthResourceController, ResourceChallenge, AudienceBinding],
)]
struct OAuthResourceHost;

/// DI module for the RFC 9728 discovery surface. See the module docs for what
/// importing it enforces.
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

    fn collect(&self, builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
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

    fn register(self, builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder.import::<OAuthResourceHost>()
    }
}

//! [`OAuthResourceModule`] — turns this app into a conformant OAuth 2.1
//! resource server.
//!
//! Imported bare — or through `for_root`, to pin a base its variables
//! overlay — it, at boot:
//!
//! 1. Validates [`OAuthResourceConfig`] into the served
//!    [`ProtectedResourceMetadata`] — advertising, when the deployment names
//!    no scope, the [`Scope`]s an import declares — and provides it as global
//!    infrastructure.
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

use std::any::TypeId;

use nest_rs_authn::__private::ResourceBinding;
use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_config::{ConfigModule, endpoint_shown};
use nest_rs_core::__private::ForRootValue;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Module, Registering, module};
use nest_rs_guards::Scope;
use nest_rs_http::__private::CanonicalHost;
use nest_rs_http::HttpModule;

use crate::config::OAuthResourceConfig;
use crate::controller::OAuthResourceController;
use crate::interceptor::ResourceChallenge;
use crate::metadata::ProtectedResourceMetadata;

/// DI module for the RFC 9728 discovery surface. Imported bare, it reads
/// `<PREFIX>_OAUTH__RESOURCE__*`; [`OAuthResourceModule::for_root`] declares
/// the scopes the app's rules gate on, or pins a base those variables overlay.
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
    /// What the app declares about this resource server, in one value: the
    /// [`Scope`]s its rules gate on, which the document and the challenge
    /// advertise as `scopes_supported` (RFC 9728 §2) when the deployment names
    /// none (`<PREFIX>_OAUTH__RESOURCE__SCOPES_SUPPORTED`); an
    /// [`OAuthResourceConfig`] pinned as the base
    /// `<PREFIX>_OAUTH__RESOURCE__*` overlays, per field; or both, the scopes
    /// before the config. The bare import declares no scope and reads those
    /// variables over [`OAuthResourceConfig`]'s defaults.
    ///
    /// ```
    /// use nest_rs_core::module;
    /// use nest_rs_guards::Scope;
    /// use nest_rs_oauth_resource::OAuthResourceModule;
    ///
    /// pub const POSTS_READ: Scope = Scope::new("posts:read");
    /// pub const POSTS_WRITE: Scope = Scope::new("posts:write");
    ///
    /// #[module(imports = [OAuthResourceModule::for_root([POSTS_READ, POSTS_WRITE])])]
    /// struct AssistantModule;
    /// ```
    pub fn for_root(declared: impl ForRootValue<OAuthResourceSetup>) -> OAuthResourceSetup {
        declared.into_setup()
    }
}

/// The configured import [`OAuthResourceModule::for_root`] returns: the
/// scopes it declares, the config it pins, or both.
///
/// `for_root` is the one way to it:
///
/// ```compile_fail,E0277
/// use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceSetup};
///
/// let second_way = OAuthResourceSetup::from(OAuthResourceConfig::default());
/// ```
pub struct OAuthResourceSetup {
    scopes: Option<Vec<Scope>>,
    config: Option<OAuthResourceConfig>,
}

impl ForRootValue<OAuthResourceSetup> for OAuthResourceConfig {
    fn into_setup(self) -> OAuthResourceSetup {
        OAuthResourceSetup {
            scopes: None,
            config: Some(self),
        }
    }
}

impl<const N: usize> ForRootValue<OAuthResourceSetup> for [Scope; N] {
    fn into_setup(self) -> OAuthResourceSetup {
        OAuthResourceSetup {
            scopes: Some(self.to_vec()),
            config: None,
        }
    }
}

impl<const N: usize> ForRootValue<OAuthResourceSetup> for ([Scope; N], OAuthResourceConfig) {
    fn into_setup(self) -> OAuthResourceSetup {
        OAuthResourceSetup {
            scopes: Some(self.0.to_vec()),
            config: Some(self.1),
        }
    }
}

/// The remedy a second declared scope set is refused with.
const ONE_SCOPE_SET: &str = "A resource server advertises one scope set — declare its scopes on one \
                             `OAuthResourceModule::for_root`, and let every other import of the \
                             module declare none.";

impl DynamicModule for OAuthResourceSetup {
    fn module() -> TypeId {
        TypeId::of::<OAuthResourceModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = builder.import::<OAuthResourceModule>();
        let builder = match self.config.clone() {
            Some(config) => ConfigModule::provide_root(config, builder),
            None => builder,
        };
        // A declaration, so a second import declaring a set fails the boot
        // naming both, before any factory runs.
        match self.scopes.clone() {
            Some(scopes) => builder.provide_declared_factory::<OAuthResourceScopes, _, _>(
                ONE_SCOPE_SET,
                move |_| async move { Ok(OAuthResourceScopes(scopes)) },
            ),
            None => builder,
        }
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<OAuthResourceModule>()
    }
}

/// The scopes an import of the module declared; none for the bare import.
#[derive(Default)]
struct OAuthResourceScopes(Vec<Scope>);

/// Queues the served document and what it binds: `#[module]` takes only
/// imports and providers, so this hand-written module carries the factories
/// for [`OAuthResourceModule`].
struct OAuthResourceWiring;

impl Module for OAuthResourceWiring {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_factory::<OAuthResourceScopes, _, _>(|_| async { Ok(OAuthResourceScopes::default()) })
            .provide_factory_after::<ProtectedResourceMetadata, (OAuthResourceConfig, OAuthResourceScopes), _, _>(
                |container| async move {
                    let config = ConfigModule::resolved::<OAuthResourceConfig>(&container)?;
                    let declared = container.get::<OAuthResourceScopes>().ok_or_else(|| {
                        anyhow::anyhow!("the declared scope set did not resolve")
                    })?;
                    frozen(advertising(&declared.0, (*config).clone()))
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

/// `config`, advertising the `declared` scopes when it names none.
fn advertising(declared: &[Scope], mut config: OAuthResourceConfig) -> OAuthResourceConfig {
    if config.scopes_supported.is_empty() {
        config.scopes_supported = declared.iter().copied().map(String::from).collect();
    }
    config
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

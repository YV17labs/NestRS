//! [`McpModule`] — the app's say over its MCP surface: the streamable-HTTP
//! server ([`McpConfig`]) and who the app is ([`McpIdentity`]).
//!
//! It activates nothing: listing an `#[mcp]` provider mounts the endpoint.
//! Without it every mount runs on [`McpConfig::default`], gated, and reports
//! what its hosts declare. Identity and posture have no `<PREFIX>_MCP__*` twin,
//! so they travel in [`McpOptions`].

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Posture, Registering, module};
use nest_rs_http::__private::EndpointPosture;

use crate::config::McpConfig;
use crate::identity::McpIdentity;
use crate::registry;
use crate::view::McpView;

/// DI module that resolves [`McpConfig`] and carries the app's own
/// [`McpIdentity`].
#[module(imports = [ConfigModule::for_feature::<McpConfig>()])]
pub struct McpModule;

impl McpModule {
    /// Everything the app says about MCP, in one value — see [`McpOptions`].
    ///
    /// An app that only pins server options passes an [`McpConfig`] (or `None`
    /// for pure environment) exactly like every other module's `for_root`.
    pub fn for_root(options: impl Into<McpOptions>) -> McpSetup {
        McpSetup {
            options: options.into(),
        }
    }
}

/// What an app declares about MCP: the server options every mount runs on, the
/// identity every endpoint reports unless one of its hosts says otherwise, and
/// who may call its endpoints.
///
/// ```
/// use nest_rs_core::module;
/// use nest_rs_mcp::{McpIdentity, McpModule, McpOptions};
/// # use nest_rs_core::Posture;
/// # use nest_rs_mcp::{DEFAULT_PATH, McpError, mcp, tools};
/// # use nest_rs_testing::mcp::{initialize, result};
///
/// #[module(imports = [
///     McpModule::for_root(McpOptions {
///         server: Some(
///             McpIdentity::new("assistant", env!("CARGO_PKG_VERSION"))
///                 .instructions("Every result is scoped to the caller's token."),
///         ),
/// #       posture: Posture::Public,
///         ..Default::default()
///     }),
/// ])]
/// struct AppModule;
/// #
/// # #[mcp]
/// # #[derive(Default)]
/// # struct PingHost;
/// #
/// # #[tools]
/// # impl PingHost {
/// #     #[tool(description = "Answer a ping.")]
/// #     #[public]
/// #     async fn ping(&self) -> Result<String, McpError> {
/// #         Ok("pong".into())
/// #     }
/// # }
/// #
/// # #[module(imports = [AppModule], providers = [PingHost])]
/// # struct ServedModule;
/// #
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = nest_rs_testing::TestApp::for_module::<ServedModule>().await?;
/// # let handshake = result(&initialize(app.http(), DEFAULT_PATH, None).await);
///
/// assert_eq!(handshake["result"]["serverInfo"]["name"], "assistant");
/// assert_eq!(
///     handshake["result"]["instructions"],
///     "Every result is scoped to the caller's token.",
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct McpOptions {
    /// The base `<PREFIX>_MCP__*` overlays, per field. `None` ⇒ the environment
    /// over [`McpConfig::default`]; a bare `McpConfig` would demote the `.env` cascade.
    pub config: Option<McpConfig>,
    /// Who this app is: the `serverInfo` every endpoint reports unless an `#[mcp]`
    /// host overrides it per field. Declared where no `#[mcp]` host serves, it fails boot.
    pub server: Option<McpIdentity>,
    /// Who may call the app's MCP endpoints. [`Posture::Gated`], the default:
    /// the global guard pool and the `EndpointGuards<McpView, _>` the app's
    /// modules declare run at each request, and with neither the endpoint
    /// denies every request. [`Posture::Public`] opens them with no guard at the
    /// request, the one visible line that does, in code alone, at one import: a
    /// second import opening them fails the boot naming both, and a module
    /// declaring `EndpointGuards` for them fails it too. Any other posture
    /// fails the boot.
    pub posture: Posture,
}

impl Default for McpOptions {
    fn default() -> Self {
        Self {
            config: None,
            server: None,
            posture: Posture::Gated,
        }
    }
}

impl From<McpConfig> for McpOptions {
    fn from(config: McpConfig) -> Self {
        Some(config).into()
    }
}

impl From<Option<McpConfig>> for McpOptions {
    fn from(config: Option<McpConfig>) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }
}

/// [`DynamicModule`] returned by [`McpModule::for_root`]: resolves
/// [`McpConfig`] (env over the pinned base) and declares the app's
/// [`McpIdentity`].
pub struct McpSetup {
    options: McpOptions,
}

/// The remedy a second declared identity is refused with.
const ONE_IDENTITY: &str = "An app reports one serverInfo — declare its identity once, on one \
                            `McpModule::for_root`, and let every other import of the module \
                            leave `server` unset.";

/// The remedy a second opening is refused with.
const ONE_OPENING: &str = "An app opens its MCP endpoints once — set `posture: Posture::Public` \
                           on one `McpModule::for_root`, and let every other import of the \
                           module leave `posture` at its default.";

/// The registry the wiring step checks the app's declared identity reaches.
const IDENTITY: &str = "nest_rs::mcp::identity";

/// The app's opening of its MCP endpoints, declared by the one import setting
/// [`Posture::Public`].
struct McpPosture;

impl DynamicModule for McpSetup {
    fn module() -> TypeId {
        TypeId::of::<McpModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(
            self.options.config.clone(),
            builder.import::<McpModule>(),
        );
        // Declarations, so a second import declaring one fails the boot naming
        // both, before any factory runs.
        let builder = if self.options.posture == Posture::Public {
            builder.provide_declared_factory::<McpPosture, _, _>(ONE_OPENING, |_| async {
                Ok(McpPosture)
            })
        } else {
            builder
        };
        match self.options.server.clone() {
            Some(server) => builder
                .provide_declared_factory::<McpIdentity, _, _>(ONE_IDENTITY, move |_| async move {
                    Ok(server)
                }),
            None => builder,
        }
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        // Gated is every endpoint's own posture: only the opening is declared, so
        // a second import of the module, a library's, cannot close what the app
        // opened, and `collect` lets one import open it.
        let builder = match self.options.posture {
            Posture::Gated => builder,
            Posture::Public => {
                builder.provide_meta(EndpointPosture::of::<McpView>(Posture::Public))
            }
            refused => builder.refuse(nest_rs_core::anyhow::anyhow!(
                "McpOptions::posture is {refused:?}, and an MCP endpoint is Posture::Gated \
                 (the default) or Posture::Public — set one of the two",
            )),
        };
        // The one import declaring the identity checks that it reaches a host.
        let builder = match self.options.server {
            Some(_) => builder.provide_wiring(IDENTITY, registry::server_reaches_a_host),
            None => builder,
        };
        builder.import::<McpModule>()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::App;

    use super::*;

    fn pinned_mcp() -> McpSetup {
        McpModule::for_root(McpConfig {
            allowed_hosts: vec!["mcp.example.com".into()],
            ..McpConfig::default()
        })
    }

    #[module(imports = [pinned_mcp()])]
    struct PinnedMcpHost;

    #[tokio::test]
    async fn for_root_pins_the_host_allowlist() {
        let app = App::builder()
            .module::<PinnedMcpHost>()
            .build()
            .await
            .expect("the pinned-config module boots");

        let cfg: Option<Arc<McpConfig>> = app.container().get();
        assert_eq!(
            cfg.expect("pinned McpConfig resolves").allowed_hosts,
            ["mcp.example.com"],
        );
    }

    #[test]
    fn a_config_only_call_site_needs_no_options_literal() {
        let unpinned: McpOptions = McpModule::for_root(None).options;
        assert!(unpinned.config.is_none());
        assert!(unpinned.server.is_none());
        assert_eq!(
            unpinned.posture,
            Posture::Gated,
            "gated unless opened in code"
        );
    }

    #[module(imports = [McpModule::for_root(McpOptions {
        posture: Posture::InBand,
        ..Default::default()
    })])]
    struct InBandMcp;

    #[tokio::test]
    async fn a_posture_other_than_gated_or_public_fails_the_boot_naming_both() {
        let Err(refused) = App::builder().module::<InBandMcp>().build().await else {
            panic!("an MCP endpoint is gated or public");
        };
        let said = format!("{refused:#}");
        assert!(
            said.contains("InBand")
                && said.contains("Posture::Gated")
                && said.contains("Posture::Public"),
            "{said}",
        );
    }
}

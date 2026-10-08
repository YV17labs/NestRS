//! [`McpModule`] — the app's say over its MCP surface: the streamable-HTTP
//! server ([`McpConfig`]) and who the app is ([`McpIdentity`]).
//!
//! It activates nothing: listing an `#[mcp]` provider mounts the endpoint.
//! Without it every mount runs on [`McpConfig::default`] and reports what its
//! hosts declare. Identity has no `<PREFIX>_MCP__*` twin, so it travels in [`McpOptions`].

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Registering, module};

use crate::config::McpConfig;
use crate::identity::McpIdentity;
use crate::registry;

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

/// What an app declares about MCP: the server options every mount runs on, and
/// the identity every endpoint reports unless one of its hosts says otherwise.
///
/// ```
/// use nest_rs_core::module;
/// use nest_rs_mcp::{McpIdentity, McpModule, McpOptions};
/// # use nest_rs_mcp::{AllowAllMcpGuard, DEFAULT_PATH, McpError, McpOperationGuard, mcp, tools};
/// # use nest_rs_testing::mcp::{initialize, result};
///
/// #[module(imports = [
///     McpModule::for_root(McpOptions {
///         server: Some(
///             McpIdentity::new("assistant", env!("CARGO_PKG_VERSION"))
///                 .instructions("Every result is scoped to the caller's token."),
///         ),
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
/// # #[module(imports = [AppModule], providers = [PingHost, AllowAllMcpGuard as dyn McpOperationGuard])]
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
#[derive(Clone, Debug, Default)]
pub struct McpOptions {
    /// The base `<PREFIX>_MCP__*` overlays, per field. `None` ⇒ the environment
    /// over [`McpConfig::default`]; a bare `McpConfig` would demote the `.env` cascade.
    pub config: Option<McpConfig>,
    /// Who this app is: the `serverInfo` every endpoint reports unless an `#[mcp]`
    /// host overrides it per field. Declared where no `#[mcp]` host serves, it fails boot.
    pub server: Option<McpIdentity>,
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
            server: None,
        }
    }
}

/// [`DynamicModule`] returned by [`McpModule::for_root`]: resolves
/// [`McpConfig`] (env over the pinned base) and provides the app's declared
/// [`McpIdentity`].
pub struct McpSetup {
    options: McpOptions,
}

impl DynamicModule for McpSetup {
    fn module() -> TypeId {
        TypeId::of::<McpModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(self.options.config.clone(), builder.import::<McpModule>())
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        // Provider-less metadata: importing this module is itself the gate.
        let builder = match self.options.server {
            Some(server) => builder
                .provide_meta(server)
                .provide_meta(registry::server_reaches_a_host())
                .provide_meta(registry::server_is_declared_once()),
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
        McpModule::for_root(McpConfig::default().with_allowed_hosts(["mcp.example.com"]))
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
    }
}

//! [`McpModule`] — the app's say over its MCP surface: the streamable-HTTP
//! server ([`McpConfig`]), who the app is ([`McpIdentity`]) and who may call it
//! ([`McpAccess`]).
//!
//! Listing an `#[mcp]` provider mounts the endpoint on the HTTP transport, and
//! brings this module bare, which reads `<PREFIX>_MCP__*` and imports
//! `HttpModule` bare in turn, so the app importing the adapter serves it on the
//! deployment's config; the root's [`McpModule::for_root`] pins a base over
//! it. Identity and access have no `<PREFIX>_MCP__*` twin, so `for_root` takes
//! them beside the config, before it.

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::__private::{ForRootValue, RuntimeModule};
use nest_rs_core::{
    Collecting, Container, ContainerBuilder, DynamicModule, Posture, Registering, module,
};
use nest_rs_http::__private::EndpointPosture;
use nest_rs_http::HttpModule;

use crate::access::McpAccess;
use crate::config::McpConfig;
use crate::identity::McpIdentity;
use crate::registry;
use crate::view::McpView;

/// DI module that resolves [`McpConfig`] — imported bare, it reads
/// `<PREFIX>_MCP__*` — and, through [`McpModule::for_root`], carries the app's
/// own [`McpIdentity`] and [`McpAccess`].
#[module(imports = [nest_rs_config::__private::feature::<McpConfig>(), HttpModule])]
pub struct McpModule;

impl McpModule {
    /// What the app declares about MCP, in one value: an [`McpConfig`] pinned
    /// as the base `<PREFIX>_MCP__*` overlays, per field; the identity every
    /// endpoint reports unless one of its hosts says otherwise, over the
    /// binary's name, version and description ([`McpIdentity`]); who may call
    /// the endpoints ([`McpAccess`]); or the declarations before the config,
    /// as a tuple — `(McpAccess, McpIdentity)`, `(McpIdentity, McpConfig)`,
    /// `(McpAccess, McpConfig)`, `(McpAccess, McpIdentity, McpConfig)`.
    ///
    /// ```
    /// use nest_rs_core::module;
    /// use nest_rs_mcp::{McpIdentity, McpModule};
    /// # use nest_rs_mcp::McpAccess;
    /// # use nest_rs_mcp::{DEFAULT_PATH, McpError, mcp, tools};
    /// # use nest_rs_testing::mcp::{initialize, result};
    ///
    /// #[module(imports = [
    ///     McpModule::for_root(
    /// #       (McpAccess::Public,
    ///         McpIdentity {
    ///             title: Some("Assistant".into()),
    ///             instructions: Some("Every result is scoped to the caller's token.".into()),
    ///             ..Default::default()
    ///         },
    /// #       )
    ///     ),
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
    /// let server = &handshake["result"]["serverInfo"];
    /// assert_eq!(server["name"], env!("CARGO_PKG_NAME"), "the binary's, by default");
    /// assert_eq!(server["version"], env!("CARGO_PKG_VERSION"));
    /// assert_eq!(server["title"], "Assistant");
    /// assert_eq!(
    ///     handshake["result"]["instructions"],
    ///     "Every result is scoped to the caller's token.",
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn for_root(declared: impl ForRootValue<McpSetup>) -> McpSetup {
        declared.into_setup()
    }
}

/// [`DynamicModule`] returned by [`McpModule::for_root`]: resolves
/// [`McpConfig`] (env over the pinned base, when one is), and declares the
/// app's [`McpIdentity`] and its opening.
///
/// `for_root` is the one way to it:
///
/// ```compile_fail,E0277
/// let second_way = nest_rs_mcp::McpSetup::from(nest_rs_mcp::McpConfig::default());
/// ```
pub struct McpSetup {
    access: Option<McpAccess>,
    server: Option<McpIdentity>,
    config: Option<McpConfig>,
}

/// The setup declaring `access`, `server` and `config`.
fn declaring(
    access: Option<McpAccess>,
    server: Option<McpIdentity>,
    config: Option<McpConfig>,
) -> McpSetup {
    McpSetup {
        access,
        server,
        config,
    }
}

impl ForRootValue<McpSetup> for McpConfig {
    fn into_setup(self) -> McpSetup {
        declaring(None, None, Some(self))
    }
}

impl ForRootValue<McpSetup> for McpIdentity {
    fn into_setup(self) -> McpSetup {
        declaring(None, Some(self), None)
    }
}

impl ForRootValue<McpSetup> for McpAccess {
    fn into_setup(self) -> McpSetup {
        declaring(Some(self), None, None)
    }
}

impl ForRootValue<McpSetup> for (McpIdentity, McpConfig) {
    fn into_setup(self) -> McpSetup {
        declaring(None, Some(self.0), Some(self.1))
    }
}

impl ForRootValue<McpSetup> for (McpAccess, McpConfig) {
    fn into_setup(self) -> McpSetup {
        declaring(Some(self.0), None, Some(self.1))
    }
}

impl ForRootValue<McpSetup> for (McpAccess, McpIdentity) {
    fn into_setup(self) -> McpSetup {
        declaring(Some(self.0), Some(self.1), None)
    }
}

impl ForRootValue<McpSetup> for (McpAccess, McpIdentity, McpConfig) {
    fn into_setup(self) -> McpSetup {
        declaring(Some(self.0), Some(self.1), Some(self.2))
    }
}

/// The remedy a second declared identity is refused with.
const ONE_IDENTITY: &str = "An app reports one serverInfo — declare its identity once, on one \
                            `McpModule::for_root`, and let every other import of the module \
                            declare none.";

/// The remedy a second opening is refused with.
const ONE_OPENING: &str = "An app opens its MCP endpoints once — pass `McpAccess::Public` to one \
                           `McpModule::for_root`, and let every other import of the module leave \
                           them gated.";

/// The registry the wiring step checks the app's declared identity reaches.
const IDENTITY: &str = "nest_rs::mcp::identity";

/// The app's opening of its MCP endpoints, declared by the one import passing
/// [`McpAccess::Public`].
struct McpOpening;

/// The posture the app's MCP endpoints run under: [`Posture::Public`] when an
/// import opened them, [`Posture::Gated`] otherwise.
pub(crate) fn posture(container: &Container) -> Posture {
    if container.get::<McpOpening>().is_some() {
        Posture::Public
    } else {
        Posture::Gated
    }
}

impl DynamicModule for McpSetup {
    fn module() -> TypeId {
        TypeId::of::<McpModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = builder.import::<McpModule>();
        let builder = match self.config.clone() {
            Some(config) => ConfigModule::provide_root(config, builder),
            None => builder,
        };
        // Declarations, so a second import declaring one fails the boot naming
        // both, before any factory runs.
        let builder = match self.access {
            Some(McpAccess::Public) => builder
                .provide_declared_factory::<McpOpening, _, _>(ONE_OPENING, |_| async {
                    Ok(McpOpening)
                }),
            None => builder,
        };
        match self.server.clone() {
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
        let builder = match self.access {
            None => builder,
            Some(McpAccess::Public) => {
                builder.provide_meta(EndpointPosture::of::<McpView>(Posture::Public))
            }
        };
        // The one import declaring the identity checks that it reaches a host.
        let builder = match self.server {
            Some(_) => builder.provide_wiring(IDENTITY, registry::server_reaches_a_host),
            None => builder,
        };
        builder.import::<McpModule>()
    }
}

/// What an `#[mcp]` host runs on, which `#[mcp]` brings: the module reading
/// the endpoint's config, which imports the transport it mounts on.
pub const RUNTIME: &[RuntimeModule] = &[RuntimeModule::of::<McpModule>()];

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
    fn each_declaration_reaches_the_setup_and_nothing_else_does() {
        let identity = McpModule::for_root(McpIdentity {
            title: Some("Assistant".into()),
            ..McpIdentity::default()
        });
        assert!(identity.server.is_some() && identity.config.is_none());
        assert_eq!(identity.access, None, "gated unless opened in code");

        let opened = McpModule::for_root((McpAccess::Public, McpConfig::default()));
        assert!(opened.server.is_none() && opened.config.is_some());
        assert_eq!(opened.access, Some(McpAccess::Public));

        let all = McpModule::for_root((
            McpAccess::Public,
            McpIdentity::default(),
            McpConfig::default(),
        ));
        assert!(all.server.is_some() && all.config.is_some());
    }
}

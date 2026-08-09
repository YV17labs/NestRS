//! [`McpModule`] — the app's say over its MCP endpoints: their configuration
//! ([`McpConfig`]) and their identity ([`McpEndpoint`]).
//!
//! **It is not an activation seam.** MCP still activates the way it always has:
//! list the `#[mcp]`-decorated provider, and the endpoint mounts itself on the
//! HTTP transport. Import this module to configure the streamable-HTTP server —
//! most importantly the `Host` allowlist a public deployment needs — and to name
//! the endpoint several features share. Without it every mount runs on
//! [`McpConfig::default`] (rmcp's own loopback-only posture) and reports its
//! first host's identity.
//!
//! [`McpConfig`] loads from `NESTRS_MCP__*` by default (importing `McpModule`
//! is enough); [`McpModule::for_root`] supplies a base for those variables to
//! overlay, so a field pinned in code is still overridable per field by the
//! deployment (see `nest_rs_config::Config`).
//!
//! Identity is **not** config: a server's name, version and instructions are
//! part of what the app *is*, the same way a GraphQL schema's root type is, so
//! they are declared in code and carry no `NESTRS_MCP__*` twin. That is why
//! [`McpOptions`] exists — the two declarations travel together into the one
//! `for_root` seam instead of the identity arriving through a second call.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, Module, module};

use crate::config::McpConfig;
use crate::identity::McpEndpoint;
use crate::registry;

/// DI module that resolves [`McpConfig`] and carries the app's declared
/// [`McpEndpoint`] identities. See the module docs for why it is optional.
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
/// the identity of each endpoint it owns.
///
/// ```no_run
/// use nest_rs_core::module;
/// use nest_rs_mcp::{McpEndpoint, McpModule, McpOptions};
///
/// #[module(imports = [
///     McpModule::for_root(McpOptions {
///         endpoints: vec![McpEndpoint::new("/mcp", "assistant", "1.0.0")],
///         ..Default::default()
///     }),
/// ])]
/// struct AppModule;
/// ```
#[derive(Clone, Debug, Default)]
pub struct McpOptions {
    /// The base `NESTRS_MCP__*` overlays, per field. `None` ⇒ the environment
    /// over [`McpConfig::default`]. It stays an `Option` because
    /// `nest_rs_config::Config::resolve` ranks the `.env` cascade *below* a
    /// pinned base and *above* the defaults — a bare `McpConfig` here would
    /// demote the cascade for apps that pinned nothing.
    pub config: Option<McpConfig>,
    /// The identity of each endpoint this app owns, one entry per path.
    /// Declaring a path no `#[mcp]` host serves fails boot — a declaration that
    /// reaches nothing is a typo, not a no-op.
    pub endpoints: Vec<McpEndpoint>,
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
            endpoints: Vec::new(),
        }
    }
}

/// [`DynamicModule`] returned by [`McpModule::for_root`]: resolves
/// [`McpConfig`] (env over the pinned base) and provides every declared
/// [`McpEndpoint`]. Queued first, so it wins over — and skips — the plain env
/// factory the base module queues.
pub struct McpSetup {
    options: McpOptions,
}

impl DynamicModule for McpSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.options.config.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        // Provider-less metadata: an identity is the *app's* declaration about a
        // path, not a role played by some provider, so there is nothing to
        // attach it to and nothing for module-gating to gate — the import of
        // this module is itself the gate.
        let builder = self
            .options
            .endpoints
            .into_iter()
            .fold(builder, ContainerBuilder::provide_meta);
        <McpModule as Module>::register(builder.provide_meta(registry::declaration_check()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::App;

    use super::*;

    /// Pinned allowlist for the `for_root` test below, as a real import site.
    fn pinned_mcp() -> McpSetup {
        McpModule::for_root(McpConfig::default().with_allowed_hosts(["mcp.example.com"]))
    }

    #[module(imports = [pinned_mcp()])]
    struct PinnedMcpHost;

    #[tokio::test]
    async fn for_root_pins_the_host_allowlist() {
        // `for_root(Some(cfg))` queues the resolving factory rather than
        // providing the struct verbatim — that is what keeps `NESTRS_MCP__*`
        // live for every field the call site did not pin — so the value
        // materializes in the AppBuilder's factory phase.
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

    /// An app that pins no identity still calls `for_root` exactly like every
    /// other module's — that is the whole reason `McpOptions` carries the
    /// conversions rather than forcing a struct literal. `pinned_mcp` above is
    /// the `Some` arm of the same claim, booted; this is the `None` one.
    #[test]
    fn a_config_only_call_site_needs_no_options_literal() {
        let unpinned: McpOptions = McpModule::for_root(None).options;
        assert!(unpinned.config.is_none());
        assert!(unpinned.endpoints.is_empty());
    }
}

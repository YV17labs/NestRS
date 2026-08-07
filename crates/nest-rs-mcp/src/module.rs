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
//! they are declared in code and carry no `NESTRS_MCP__*` twin.

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
    /// `None` ⇒ load [`McpConfig`] from `NESTRS_MCP__*` over its defaults;
    /// `Some(cfg)` makes `cfg` the base those variables overlay.
    ///
    /// Chain [`McpSetup::endpoint`] to also declare an endpoint's identity.
    pub fn for_root(config: impl Into<Option<McpConfig>>) -> McpSetup {
        McpSetup {
            pinned: config.into(),
            endpoints: Vec::new(),
        }
    }

    /// Declare an endpoint's identity, leaving [`McpConfig`] to the environment
    /// — shorthand for `for_root(None).endpoint(..)`, which is the common shape
    /// for an app that names its endpoint but pins no server option.
    pub fn endpoint(endpoint: McpEndpoint) -> McpSetup {
        Self::for_root(None::<McpConfig>).endpoint(endpoint)
    }
}

/// [`DynamicModule`] returned by [`McpModule::for_root`]: resolves
/// [`McpConfig`] (env over the pinned base) and provides every declared
/// [`McpEndpoint`]. Queued first, so it wins over — and skips — the plain env
/// factory the base module queues.
pub struct McpSetup {
    pinned: Option<McpConfig>,
    endpoints: Vec<McpEndpoint>,
}

impl McpSetup {
    /// Declare the identity of the endpoint at one path. Call once per path an
    /// app owns; declaring a path no `#[mcp]` host serves fails boot.
    pub fn endpoint(mut self, endpoint: McpEndpoint) -> Self {
        self.endpoints.push(endpoint);
        self
    }
}

impl DynamicModule for McpSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        // Provider-less metadata: an identity is the *app's* declaration about a
        // path, not a role played by some provider, so there is nothing to
        // attach it to and nothing for module-gating to gate — the import of
        // this module is itself the gate.
        let builder = self
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
}

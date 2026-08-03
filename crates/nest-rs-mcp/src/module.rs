//! [`McpModule`] — resolves [`McpConfig`] for every `#[mcp]` mount in the app.
//!
//! **It is not an activation seam.** MCP still activates the way it always has:
//! list the `#[mcp]`-decorated provider, and the endpoint mounts itself on the
//! HTTP transport. Import this module only to configure the streamable-HTTP
//! server — most importantly the `Host` allowlist a public deployment needs.
//! Without it every mount runs on [`McpConfig::default`], which is rmcp's own
//! loopback-only posture.
//!
//! [`McpConfig`] loads from `NESTRS_MCP__*` by default (importing `McpModule`
//! is enough); [`McpModule::for_root`] supplies a base for those variables to
//! overlay, so a field pinned in code is still overridable per field by the
//! deployment (see `nest_rs_config::Config`).

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, Module, module};

use crate::config::McpConfig;

/// DI module that resolves [`McpConfig`]. See the module docs for why it is
/// optional.
#[module(imports = [ConfigModule::for_feature::<McpConfig>()])]
pub struct McpModule;

impl McpModule {
    /// `None` ⇒ load [`McpConfig`] from `NESTRS_MCP__*` over its defaults;
    /// `Some(cfg)` makes `cfg` the base those variables overlay.
    pub fn for_root(config: impl Into<Option<McpConfig>>) -> McpSetup {
        McpSetup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`McpModule::for_root`]: resolves
/// [`McpConfig`] (env over the pinned base). Queued first, so it wins over —
/// and skips — the plain env factory the base module queues.
pub struct McpSetup {
    pinned: Option<McpConfig>,
}

impl DynamicModule for McpSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        <McpModule as Module>::register(builder)
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

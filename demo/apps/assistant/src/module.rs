use features::audio::AudioMcpModule;
use features::posts::PostsMcpModule;
use nest_rs::authn::ProtectedResourceModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::mcp::McpModule;
use nest_rs::redis::QueueModule;
use nest_rs::seaorm::DatabaseModule;
use nest_rs::server_timing::ServerTimingModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        ServerTimingModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
        // Resolves `NESTRS_MCP__*` for every `#[mcp]` mount — the `Host`
        // allowlist above all, which is loopback-only until a deployment names
        // itself. Importing it activates nothing: the tool hosts below are what
        // mount the endpoints.
        McpModule,
        // This app is an OAuth 2.1 resource server, so it says so the way the
        // MCP authorization spec requires: `/.well-known/oauth-protected-resource`
        // is served, and every 401 points at it. Configured through
        // `NESTRS_AUTHN__*` — a deployment names its own canonical URI and its
        // issuer, so there is nothing here worth pinning in code.
        ProtectedResourceModule::for_root(None),
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        AudioMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;

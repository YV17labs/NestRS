use features::audio::AudioMcpModule;
use features::posts::PostsMcpModule;
use features::users::UsersMcpModule;
use nest_rs::authn::ProtectedResourceModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::mcp::{McpEndpoint, McpModule, McpOptions};
use nest_rs::redis::QueueModule;
use nest_rs::seaorm::DatabaseModule;
use nest_rs::server_timing::ServerTimingModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        ServerTimingModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
        McpModule::for_root(McpOptions {
            endpoints: vec![
                McpEndpoint::new("/mcp", "nestrs-assistant", env!("CARGO_PKG_VERSION"))
                    .title("nestrs demo assistant")
                    .instructions(
                        "Tools over the demo's own data. `transcribe_audio` queues a \
                         transcription and returns its job id; `list_people` reads the \
                         directory. Every call is scoped to the caller's token — an \
                         empty result means not authorized, not absent.",
                    ),
                McpEndpoint::new("/posts/mcp", "nestrs-assistant-posts", env!("CARGO_PKG_VERSION"))
                    .title("nestrs demo assistant — posts"),
            ],
            ..Default::default()
        }),
        ProtectedResourceModule::for_root(None),
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        AudioMcpModule,
        UsersMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;

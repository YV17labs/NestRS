use features::audio::AudioMcpModule;
use features::posts::PostsMcpModule;
use features::users::UsersMcpModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::mcp::{McpIdentity, McpModule, McpOptions};
use nest_rs::oauth::resource::OAuthResourceModule;
use nest_rs::opentelemetry::OpenTelemetryModule;
use nest_rs::redis::{RedisModule, RedisQueueModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::server_timing::ServerTimingModule;

const SERVER_TITLE: &str = "nestrs demo assistant";
const INSTRUCTIONS: &str = "Tools over the demo's own data. Every call is scoped to the \
                            caller's token — an empty result means not authorized, not \
                            absent, never that the record does not exist. What each tool \
                            does is in its own description.";

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        ServerTimingModule,
        HealthModule,
        McpModule::for_root(McpOptions {
            server: Some(
                McpIdentity::default()
                    .title(SERVER_TITLE)
                    .instructions(INSTRUCTIONS),
            ),
            ..Default::default()
        }),
        OAuthResourceModule::for_root(None),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        AudioMcpModule,
        UsersMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;

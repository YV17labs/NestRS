use features::audio::AudioMcpModule;
use features::posts::PostsMcpModule;
use features::users::UsersMcpModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::mcp::{McpIdentity, McpModule, McpOptions};
use nest_rs::oauth::resource::OAuthResourceModule;
use nest_rs::redis::RedisQueueModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule};
use nest_rs::server_timing::ServerTimingModule;

const SERVER_NAME: &str = "nestrs-assistant";
const SERVER_TITLE: &str = "nestrs demo assistant";
const INSTRUCTIONS: &str = "Tools over the demo's own data. Every call is scoped to the \
                            caller's token — an empty result means not authorized, not \
                            absent, never that the record does not exist. What each tool \
                            does is in its own description.";

#[module(
    imports = [
        ConfigModule::for_root(),
        ServerTimingModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
        McpModule::for_root(McpOptions {
            server: Some(
                McpIdentity::new(SERVER_NAME, env!("CARGO_PKG_VERSION"))
                    .title(SERVER_TITLE)
                    .instructions(INSTRUCTIONS),
            ),
            ..Default::default()
        }),
        OAuthResourceModule::for_root(None),
        SeaOrmDatabaseModule::for_root(None),
        SeaOrmHealthModule,
        RedisQueueModule::for_root(None),
        AudioMcpModule,
        UsersMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;

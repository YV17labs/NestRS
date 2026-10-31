use features::audio::AudioMcpModule;
use features::authz::constants;
use features::posts::PostsMcpModule;
use features::users::UsersMcpModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::mcp::{McpIdentity, McpModule};
use nest_rs::oauth::resource::OAuthResourceModule;
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
        ServerTimingModule,
        HealthModule,
        McpModule::for_root(
            McpIdentity::default()
                .title(SERVER_TITLE)
                .instructions(INSTRUCTIONS),
        ),
        OAuthResourceModule::for_root(constants::ALL),
        SeaOrmModule,
        SeaOrmDatabaseModule,
        RedisModule,
        RedisQueueModule,
        AudioMcpModule,
        UsersMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;

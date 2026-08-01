use features::audio::AudioMcpModule;
use features::posts::PostsMcpModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::redis::QueueModule;
use nest_rs::seaorm::DatabaseModule;
use nest_rs::server_timing::ServerTimingModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        ServerTimingModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        AudioMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;

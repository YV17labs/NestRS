use nest_rs::core::module;
use nest_rs::redis::RedisModule;
use nest_rs::storage::StorageModule;

use super::guard::TranscodeGuard;
use super::service::AudioService;
use super::transcodes::Transcodes;

#[module(
    imports = [StorageModule, RedisModule::for_root((Transcodes, None))],
    providers = [AudioService, TranscodeGuard],
)]
pub struct AudioModule;

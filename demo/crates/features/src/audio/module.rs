use nest_rs::core::module;
use nest_rs::redis::RedisModule;
use nest_rs::storage::StorageModule;

use super::service::AudioService;
use super::transcodes::Transcodes;

#[module(
    imports = [StorageModule, RedisModule::<Transcodes>],
    providers = [AudioService],
)]
pub struct AudioModule;

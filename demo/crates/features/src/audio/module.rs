use nest_rs::core::module;
use nest_rs::storage::StorageModule;

use super::guard::TranscodeGuard;
use super::service::AudioService;

#[module(imports = [StorageModule], providers = [AudioService, TranscodeGuard])]
pub struct AudioModule;

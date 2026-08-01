use nest_rs::core::module;
use nest_rs::storage::StorageModule;

use super::service::AudioService;

#[module(imports = [StorageModule], providers = [AudioService])]
pub struct AudioModule;

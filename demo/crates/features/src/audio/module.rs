use nest_rs_core::module;
use nest_rs_storage::StorageModule;

use super::service::AudioService;

#[module(imports = [StorageModule], providers = [AudioService])]
pub struct AudioModule;

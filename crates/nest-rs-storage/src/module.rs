//! Wires the shared [`Storage`] provider and its [`StorageConfig`].
//!
//! `Storage` is built lazily on first use (see [`Storage`]), so the module only
//! has to register the provider and feed it the config loaded from
//! `NESTRS_STORAGE__*`.

use nest_rs_config::ConfigModule;
use nest_rs_core::module;

use crate::client::Storage;
use crate::config::StorageConfig;

#[module(
    imports = [ConfigModule::for_feature::<StorageConfig>()],
    providers = [Storage],
)]
pub struct StorageModule;

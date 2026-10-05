use nest_rs::core::module;
use nest_rs::queue::QueueModule;
use nest_rs::redis::{RedisModule, RedisQueueModule};

use crate::probe::Probe;
use crate::processor::{C1Processor, C16Processor};

/// One app for both roles: the parent builds it to push and never runs it, so
/// its worker transport never starts; a replica builds it and runs it.
#[module(
    imports = [
        RedisModule::for_root(None),
        RedisQueueModule,
        QueueModule::for_root(None),
    ],
    providers = [Probe, C1Processor, C16Processor],
)]
pub struct BenchModule;

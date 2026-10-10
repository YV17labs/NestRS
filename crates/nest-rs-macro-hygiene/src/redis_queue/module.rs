//! `#[module]` importing the Redis connection and the queue's binding over it,
//! both reached through `nest_rs::redis`.

use nest_rs::core::module;
use nest_rs::redis::{RedisModule, RedisQueueModule};

#[module(imports = [RedisModule::for_root(None), RedisQueueModule])]
pub struct MacroHygieneRedisQueueModule;

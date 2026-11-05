//! `#[module]` importing the Redis connection and the queue's binding over it,
//! both reached through `nest_rs::redis`.

use nest_rs::core::module;
use nest_rs::redis::RedisQueueModule;

#[module(imports = [RedisQueueModule])]
pub struct MacroHygieneRedisQueueModule;

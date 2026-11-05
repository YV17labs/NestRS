//! `#[module]` importing the queue's Redis binding, reached through
//! `nest_rs::redis`.

use nest_rs::core::module;
use nest_rs::redis::RedisQueueModule;

#[module(imports = [RedisQueueModule])]
pub struct MacroHygieneRedisQueueModule;

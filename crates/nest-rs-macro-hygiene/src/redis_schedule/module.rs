//! `#[module]` importing the Redis connection and the schedule's binding over
//! it, both reached through `nest_rs::redis`.

use nest_rs::core::module;
use nest_rs::redis::{RedisModule, RedisScheduleModule};

#[module(imports = [RedisModule, RedisScheduleModule])]
pub struct MacroHygieneRedisScheduleModule;

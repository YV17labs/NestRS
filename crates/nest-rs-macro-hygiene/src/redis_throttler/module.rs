//! `#[module]` importing the Redis connection and the rate limiter's binding
//! over it, both reached through `nest_rs::redis`.

use nest_rs::core::module;
use nest_rs::redis::RedisThrottlerModule;

#[module(imports = [RedisThrottlerModule])]
pub struct MacroHygieneRedisThrottlerModule;

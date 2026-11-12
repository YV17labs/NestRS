//! `#[module]` importing the rate limiter's Redis binding, reached through
//! `nest_rs::redis`.

use nest_rs::core::module;
use nest_rs::redis::RedisThrottlerModule;

#[module(imports = [RedisThrottlerModule])]
pub struct MacroHygieneRedisThrottlerModule;

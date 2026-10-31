//! `#[module]` importing the Redis connection and the probes, each reached
//! through the umbrella: the connection's indicator needs no import of its own.

use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::redis::RedisModule;

#[module(imports = [RedisModule, HealthModule])]
pub struct MacroHygieneRedisHealthModule;

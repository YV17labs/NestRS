//! `#[module]` importing the Redis connection, the probes and the instance's
//! indicator over it, each reached through the umbrella.

use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::redis::{RedisHealthModule, RedisModule};

#[module(imports = [RedisModule::for_root(None), HealthModule, RedisHealthModule])]
pub struct MacroHygieneRedisHealthModule;

//! `#[module]` importing the schedule's Redis binding, reached through
//! `nest_rs::redis`.

use nest_rs::core::module;
use nest_rs::redis::RedisScheduleModule;

#[module(imports = [RedisScheduleModule])]
pub struct MacroHygieneRedisScheduleModule;

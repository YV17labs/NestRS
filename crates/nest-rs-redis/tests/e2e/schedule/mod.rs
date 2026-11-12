//! The occurrence lock `RedisScheduleModule` binds, against a live Redis:
//! [`lock`] for its claims, [`module`] for what the binding refuses at boot.

mod lock;
mod module;

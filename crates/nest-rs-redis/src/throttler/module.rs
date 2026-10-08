//! [`RedisThrottlerModule`] — the Redis binding of the throttler port. A bare
//! import beside `ThrottlerModule::for_root(cfg)` (the policy and the guard) and
//! [`RedisModule::for_root`](crate::RedisModule::for_root) (the connection): it
//! declares [`RedisThrottler`] as the `dyn ThrottlerStore`, which supersedes the
//! port's in-process default wherever the three fall in `imports`. Enabled by
//! the `throttler` feature.

use std::sync::Arc;

use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering};
use nest_rs_throttler::ThrottlerStore;

use crate::RedisConnection;
use crate::throttler::RedisThrottler;

/// Cross-process rate-limit store. Import beside `ThrottlerModule::for_root`
/// to share the counters across every instance of the app; the policy
/// (`<PREFIX>_THROTTLER__*`) and the `ThrottlerGuard` stay the port's.
pub struct RedisThrottlerModule;

impl Module for RedisThrottlerModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        // Declared: it supersedes the port's in-memory default, and a second
        // vendor binding contests it by name (`BACKEND_REMEDY`).
        RedisConnection::netted(builder, "the rate limiter", nest_rs_throttler::HIT_TIMEOUT)
            .provide_declared_factory_after::<Arc<dyn ThrottlerStore>, RedisConnection, _, _>(
            nest_rs_throttler::BACKEND_REMEDY,
            |container| async move {
                let conn = RedisConnection::of(&container, "RedisThrottlerModule")?;
                Ok(Arc::new(RedisThrottler::new(conn)) as Arc<dyn ThrottlerStore>)
            },
        )
    }
}

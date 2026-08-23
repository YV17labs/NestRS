//! [`RedisThrottlerModule`] — the Redis binding of the throttler port. A bare
//! import beside `ThrottlerModule::for_root(cfg)` (the policy and the guard) and
//! [`RedisModule::for_root`](crate::RedisModule::for_root) (the connection): it
//! declares [`RedisThrottler`] as the `dyn ThrottlerStore`, which supersedes the
//! port's in-process default wherever the three fall in `imports`. Enabled by
//! the `throttler` feature.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{ContainerBuilder, Module};
use nest_rs_throttler::ThrottlerStore;

use crate::RedisConnection;
use crate::connection::CONNECTION_REMEDY;
use crate::throttler::RedisThrottler;

/// Cross-process rate-limit store. Import beside `ThrottlerModule::for_root`
/// to share the counters across every instance of the app; the policy
/// (`NESTRS_THROTTLER__*`) and the `ThrottlerGuard` stay the port's.
pub struct RedisThrottlerModule;

impl Module for RedisThrottlerModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        // Deduped like a `#[module]` expansion: a diamond import of this binding
        // is one declaration, not two contesting ones.
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        // Declared: it supersedes the port's ordinary in-memory factory, and a
        // second vendor binding contests it by name (`BACKEND_REMEDY`). Queued
        // after the connection's factory, so `imports` order is not a wiring
        // mistake a reader has to know about.
        builder.provide_declared_factory_after::<Arc<dyn ThrottlerStore>, RedisConnection, _, _>(
            nest_rs_throttler::BACKEND_REMEDY,
            |container| async move {
                let conn = container
                    .get::<RedisConnection>()
                    .ok_or_else(|| anyhow::anyhow!("RedisThrottlerModule: {CONNECTION_REMEDY}"))?;
                Ok(Arc::new(RedisThrottler::new((*conn).clone())) as Arc<dyn ThrottlerStore>)
            },
        )
    }
}

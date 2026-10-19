//! [`RedisHealthModule`] — the Redis binding of the health port. A bare import
//! beside `HealthModule` and [`RedisModule::for_root`](crate::RedisModule::for_root):
//! it attaches the instance's indicator, which runs `PING` on its connection on
//! the readiness and startup probes. Enabled by the `health` feature.

use std::marker::PhantomData;

use nest_rs_core::{ContainerBuilder, Instance, Module, Registering, Unnamed};
use nest_rs_health::{IndicatorEntry, ProbeKind};

use crate::RedisConnection;
use crate::health::indicator::{name, ping};

/// Gates the readiness and startup probes on a `PING` to one instance: the
/// default one, answering as `redis`, or a named one when imported as
/// `RedisHealthModule::<Cache>`, answering as `redis:cache`. Liveness never
/// waits on it: a Valkey outage is no reason to restart the process.
pub struct RedisHealthModule<I = Unnamed>(PhantomData<fn() -> I>);

impl<I: Instance> Module for RedisHealthModule<I> {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        if let Err(missing) = RedisConnection::<I>::of(&builder.snapshot(), "RedisHealthModule") {
            return builder.refuse(missing);
        }
        let name = name::<I>();
        builder
            .provide_meta(IndicatorEntry::new::<Self>(
                name.clone(),
                ProbeKind::Readiness,
                ping::<I>,
            ))
            .provide_meta(IndicatorEntry::new::<Self>(
                name,
                ProbeKind::Startup,
                ping::<I>,
            ))
    }
}

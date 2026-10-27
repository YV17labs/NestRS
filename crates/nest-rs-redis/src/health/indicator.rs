//! The instance's indicator: `PING` on its connection, under the crate's name
//! and the instance's — `redis`, `redis:cache`.

use std::borrow::Cow;

use nest_rs_core::{Container, ContainerBuilder, Instance, Module};
use nest_rs_health::{IndicatorEntry, IndicatorFuture, ProbeKind};

use crate::RedisConnection;

/// `builder` with the instance `I`'s indicator on the readiness and startup
/// probes, attached by `M`, the module opening its connection — so an app
/// importing `HealthModule` checks it, and liveness never waits on it: a
/// Valkey outage is no reason to restart the process.
pub(crate) fn register<I: Instance, M: Module>(builder: ContainerBuilder) -> ContainerBuilder {
    let name = name::<I>();
    builder
        .provide_meta(IndicatorEntry::new::<M>(
            name.clone(),
            ProbeKind::Readiness,
            ping::<I>,
        ))
        .provide_meta(IndicatorEntry::new::<M>(
            name,
            ProbeKind::Startup,
            ping::<I>,
        ))
}

/// The probe body's key for the instance `I`: the crate owning the connection,
/// then `:` and the instance's name for a named one.
fn name<I: Instance>() -> Cow<'static, str> {
    if I::NAME.is_empty() {
        Cow::Borrowed("redis")
    } else {
        Cow::Owned(format!("redis:{}", I::NAME))
    }
}

/// `PING` the instance `I`'s connection, which answers or fails within its
/// budget.
fn ping<I: Instance>(container: &Container) -> IndicatorFuture<'_> {
    Box::pin(async move {
        let mut conn = RedisConnection::<I>::of(container, "the Redis indicator")?;
        redis::cmd("PING").query_async::<()>(&mut conn).await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Unnamed;

    use super::*;

    #[nest_rs_core::instance]
    struct Cache;

    #[test]
    fn the_default_instance_answers_under_the_crate_s_name_and_a_named_one_after_it() {
        assert_eq!(name::<Unnamed>(), "redis");
        assert_eq!(name::<Cache>(), "redis:cache");
    }
}

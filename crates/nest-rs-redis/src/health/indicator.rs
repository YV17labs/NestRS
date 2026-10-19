//! The instance's indicator: `PING` on its connection, under the crate's name
//! and the instance's — `redis`, `redis:cache`.

use std::borrow::Cow;

use nest_rs_core::{Container, Instance};
use nest_rs_health::IndicatorFuture;

use crate::RedisConnection;

/// The probe body's key for the instance `I`: the crate owning the connection,
/// then `:` and the instance's name for a named one.
pub(crate) fn name<I: Instance>() -> Cow<'static, str> {
    if I::NAME.is_empty() {
        Cow::Borrowed("redis")
    } else {
        Cow::Owned(format!("redis:{}", I::NAME))
    }
}

/// `PING` the instance `I`'s connection, which answers or fails within its
/// budget.
pub(crate) fn ping<I: Instance>(container: &Container) -> IndicatorFuture<'_> {
    Box::pin(async move {
        let mut conn = RedisConnection::<I>::of(container, "RedisHealthModule")?;
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

//! Discovery metadata attached by `#[cron_job]`.

use std::future::Future;
use std::pin::Pin;

use nestrs_core::Container;

use crate::Trigger;

/// The thunk `#[cron_job]` generates: build the job from the container and run it
/// once. Borrows the container for the duration of the call.
pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// Discovery metadata attached by `#[cron_job]`. The [`Scheduler`](crate::Scheduler)
/// reads these via `DiscoveryService::meta::<CronJobMeta>()` at boot and runs each
/// `run` on its [`Trigger`]. Fields are `pub` only so the generated code can build it.
pub struct CronJobMeta {
    pub name: &'static str,
    pub trigger: Trigger,
    pub run: RunFn,
}

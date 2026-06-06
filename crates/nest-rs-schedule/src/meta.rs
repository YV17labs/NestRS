use std::future::Future;
use std::pin::Pin;

use nest_rs_core::Container;

use crate::Trigger;

pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

pub struct CronJobMeta {
    pub name: &'static str,
    pub trigger: Trigger,
    pub run: RunFn,
}

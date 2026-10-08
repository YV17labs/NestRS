//! [`Budget`] — what a resource waits, at most, for one answer, declared by the
//! module that opens it and held below every [`Net`](crate::Net) reaching it.

use std::any::{Any, TypeId};
use std::time::Duration;

use crate::Container;

/// What a resource waits, at most, for one answer: a pool for a connection, a
/// client for a reply. Past it the resource fails with its own error and its
/// cause.
///
/// Declared at collect (`builder.provide_meta(Budget::of::<R>(…))`) by the
/// module that opens the resource and by each binding that hands it on; declared
/// twice, it is one budget. The boot reads it as soon as the provider exists; a
/// budget whose provider was never built is not held.
pub struct Budget {
    resource: &'static str,
    setting: String,
    provider: TypeId,
    ambient: bool,
    read: Reader,
}

/// Reads a budget off the provider a container holds.
type Reader = Box<dyn Fn(&Container) -> Option<Duration> + Send + Sync>;

impl Budget {
    /// The budget of the resource `R`, as `read` finds it on the provider.
    ///
    /// `resource` names it in a sentence (`"the Redis connection"`); `setting`
    /// is what lowers it, as the refusal says it after "lower" —
    /// `` "<PREFIX>_REDIS__CONNECT_TIMEOUT_SECS, or `RedisConfig::connect_timeout` in code" ``.
    /// `read` answers `None` for a provider that holds nothing to wait on.
    pub fn of<R: Any + Send + Sync>(
        resource: &'static str,
        setting: impl Into<String>,
        read: fn(&R) -> Option<Duration>,
    ) -> Self {
        Self {
            resource,
            setting: setting.into(),
            provider: TypeId::of::<R>(),
            ambient: false,
            read: Box::new(move |container| container.get::<R>().and_then(|r| read(&r))),
        }
    }

    /// [`of`](Self::of) for a resource every unit of work reaches through the
    /// context it runs in rather than by injection — the SeaORM pool behind
    /// `Repo` — so every net around developer code reaches it.
    pub fn ambient<R: Any + Send + Sync>(
        resource: &'static str,
        setting: impl Into<String>,
        read: fn(&R) -> Option<Duration>,
    ) -> Self {
        Self {
            ambient: true,
            ..Self::of(resource, setting, read)
        }
    }

    /// The resource, as a sentence names it.
    pub(crate) fn resource(&self) -> &'static str {
        self.resource
    }

    /// What lowers the budget.
    pub(crate) fn setting(&self) -> &str {
        &self.setting
    }

    /// The provider the budget is read off.
    pub(crate) fn provider(&self) -> TypeId {
        self.provider
    }

    /// Whether every unit of work reaches the resource through its context.
    pub(crate) fn is_ambient(&self) -> bool {
        self.ambient
    }

    /// The budget the provider in `container` holds, or `None` when the boot
    /// never built it or it holds nothing to wait on.
    pub(crate) fn wait(&self, container: &Container) -> Option<Duration> {
        (self.read)(container)
    }
}

impl std::fmt::Debug for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("resource", &self.resource)
            .field("setting", &self.setting)
            .field("ambient", &self.ambient)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pool(Option<Duration>);

    fn budget() -> Budget {
        Budget::of::<Pool>("the pool", "POOL_WAIT", |pool| pool.0)
    }

    #[test]
    fn a_budget_is_read_off_the_provider_the_container_holds() {
        let container = Container::builder()
            .provide(Pool(Some(Duration::from_secs(7))))
            .build();
        assert_eq!(budget().wait(&container), Some(Duration::from_secs(7)));
    }

    #[test]
    fn a_budget_whose_provider_is_absent_or_holds_nothing_reads_none() {
        assert_eq!(budget().wait(&Container::builder().build()), None);
        let empty = Container::builder().provide(Pool(None)).build();
        assert_eq!(budget().wait(&empty), None);
    }
}

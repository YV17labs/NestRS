//! The kernel's **close** step: each resource a module opened is closed once,
//! after every shutdown hook, the one installed last first, within what the
//! hooks left of their budget.
//!
//! A module attaches its close with
//! [`ContainerBuilder::provide_close`](crate::ContainerBuilder::provide_close).
//! Every way an app ends — [`App::run`](crate::App::run) returning,
//! [`App::close`](crate::App::close), a boot that failed — drains them, so a
//! pool says goodbye to its server rather than letting its sockets fall when
//! the process exits.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::time::Instant;

use crate::container::Container;
use crate::discovery::Discovery;
use crate::millis::millis;
use crate::panic;

type Closing = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

/// Resolve the resource from the final container and start closing it; `None`
/// when nothing provides it.
type Close = Box<dyn Fn(&Container) -> Option<Closing> + Send + Sync>;

/// A close a module attached with
/// [`ContainerBuilder::provide_close`](crate::ContainerBuilder::provide_close)
/// — readable through [`Discovery::meta`], built only by the kernel; its name
/// is the resource's short type name.
pub struct CloseContribution {
    name: String,
    resource: TypeId,
    close: Close,
}

impl CloseContribution {
    pub(crate) fn new<T, F, Fut>(close: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(Arc<T>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        Self {
            name: crate::type_name::short_type_name::<T>(),
            resource: TypeId::of::<T>(),
            close: Box::new(move |container| {
                let resource = container.get::<T>()?;
                Some(Box::pin(close(resource)))
            }),
        }
    }

    /// The resource this close ends, as the lines naming it say it
    /// (`"DatabaseConnection"`).
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// What the close step reads off the container's registrations: the order
/// each type was installed in, and what the app handed in rather than a module
/// opening it.
#[derive(Default)]
pub(crate) struct InstallRecord {
    order: HashMap<TypeId, usize>,
    handed_in: HashSet<TypeId>,
}

impl InstallRecord {
    /// `installed` in the order the types were installed.
    pub(crate) fn new(installed: &[TypeId], handed_in: HashSet<TypeId>) -> Self {
        Self {
            order: installed
                .iter()
                .enumerate()
                .map(|(position, id)| (*id, position))
                .collect(),
            handed_in,
        }
    }
}

/// Run every attached close against `container`, the resource installed last
/// first — attach order breaking a tie — under `deadline`: an error is filed
/// at `error`, a panic is contained, a close still waiting at the deadline is
/// abandoned at `warn`, and every later close still starts, polled once. A
/// resource the app was handed (a seed or a test's override) is the app's to
/// close, never the kernel's.
pub(crate) async fn close_all(container: &Container, installed: &InstallRecord, deadline: Instant) {
    let mut contributions = Discovery::new(container).meta::<CloseContribution>();
    // Not attach order: a module attaches its close as it collects, before
    // the factories run in the order their inputs decide.
    contributions
        .sort_by_key(|contribution| installed.order.get(&contribution.meta.resource).copied());
    for contribution in contributions.iter().rev() {
        let contribution = &contribution.meta;
        let close = contribution.name();
        if installed.handed_in.contains(&contribution.resource) {
            tracing::debug!(
                target: crate::target::LIFECYCLE,
                close,
                "resource close skipped: the app was handed this value, so it is the app's to close",
            );
            continue;
        }
        let Some(closing) = (contribution.close)(container) else {
            tracing::debug!(
                target: crate::target::LIFECYCLE,
                close,
                "resource close skipped: nothing provides it",
            );
            continue;
        };
        tracing::debug!(target: crate::target::LIFECYCLE, close, "closing resource");
        crate::way_down::close(close);
        let started = Instant::now();
        // `timeout_at` polls the close before its timer, so past the deadline
        // it still runs once.
        match tokio::time::timeout_at(deadline, panic::contain(closing)).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => tracing::error!(
                target: crate::target::LIFECYCLE,
                close,
                error = %crate::error_message(&*err),
                "resource close failed",
            ),
            Ok(Err(payload)) => crate::contained_panic!(
                target: crate::target::LIFECYCLE,
                payload.as_ref(),
                "resource close panicked, and the closes after it still run",
                close,
            ),
            Err(_) => tracing::warn!(
                target: crate::target::LIFECYCLE,
                close,
                waited_ms = millis(started.elapsed()),
                budget_ms =
                    millis(crate::SHUTDOWN_HOOKS_TIMEOUT),
                "resource close abandoned: the shutdown hooks' budget was spent while it waited, \
                 and the closes after it still start",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use nest_rs_testing::LogCapture;

    use super::*;

    struct Pool;
    struct Cache;
    struct Seeded;

    static CLOSED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    fn closed(name: &'static str) {
        CLOSED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(name);
    }

    #[tokio::test(start_paused = true)]
    async fn closes_run_last_installed_first_and_skip_what_the_app_was_handed_or_lacks() {
        let logs = LogCapture::install();
        let builder = Container::builder()
            .provide(Pool)
            .provide(Cache)
            .provide(Seeded)
            .provide_close::<Cache, _, _>(|_| async {
                closed("Cache");
                Ok(())
            })
            .provide_close::<Seeded, _, _>(|_| async {
                closed("Seeded");
                Ok(())
            })
            .provide_close::<Pool, _, _>(|_| async {
                closed("Pool");
                Ok(())
            })
            .provide_close::<u8, _, _>(|_| async {
                closed("u8");
                Ok(())
            });
        let installed = InstallRecord::new(
            &[
                TypeId::of::<Pool>(),
                TypeId::of::<Cache>(),
                TypeId::of::<Seeded>(),
            ],
            HashSet::from([TypeId::of::<Seeded>()]),
        );
        let container = builder.build().expect("the container builds");

        close_all(
            &container,
            &installed,
            Instant::now() + crate::SHUTDOWN_HOOKS_TIMEOUT,
        )
        .await;

        assert_eq!(
            *CLOSED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["Cache", "Pool"],
        );
        let absent = logs.expect_one(
            crate::target::LIFECYCLE,
            "resource close skipped: nothing provides it",
        );
        assert_eq!(absent.field("close").as_deref(), Some("u8"));
        let handed = logs.expect_one(
            crate::target::LIFECYCLE,
            "resource close skipped: the app was handed this value, so it is the app's to close",
        );
        assert_eq!(handed.field("close").as_deref(), Some("Seeded"));
    }

    #[tokio::test]
    async fn a_close_that_fails_or_panics_is_filed_and_the_next_still_runs() {
        let logs = LogCapture::install();
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let after = Arc::clone(&ran);
        let container = Container::builder()
            .provide(Pool)
            .provide(Cache)
            .provide(Seeded)
            .provide_close::<Pool, _, _>(move |_| {
                let after = Arc::clone(&after);
                async move {
                    after.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }
            })
            .provide_close::<Cache, _, _>(|_| async { panic!("the socket was already gone") })
            .provide_close::<Seeded, _, _>(|_| async {
                Err(anyhow::anyhow!("the server refused the goodbye"))
            })
            .build()
            .expect("the container builds");

        close_all(
            &container,
            &InstallRecord::default(),
            Instant::now() + crate::SHUTDOWN_HOOKS_TIMEOUT,
        )
        .await;

        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
        let failed = logs.expect_one(crate::target::LIFECYCLE, "resource close failed");
        assert_eq!(failed.level, "error");
        assert_eq!(failed.field("close").as_deref(), Some("Seeded"));
        let panicked = logs.expect_one(
            crate::target::LIFECYCLE,
            "resource close panicked, and the closes after it still run",
        );
        assert_eq!(panicked.field("close").as_deref(), Some("Cache"));
    }
}

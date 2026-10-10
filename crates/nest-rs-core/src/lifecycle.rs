//! Lifecycle hooks: `#[hooks]` submits methods to a link-time `inventory`
//! registry that [`crate::App::run`] drains per phase, in `(provider, method)` order.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::time::Duration;

use futures_util::FutureExt as _;
use tokio::time::Instant;

use crate::container::Container;
use crate::way_down::WayDown;

/// How long the shutdown hooks may run, all of them together, before what still
/// waits is abandoned.
///
/// **One budget for the whole teardown, never one per hook.** The way down
/// spends a Kubernetes pod's default 30 s grace in steps, each bounded:
///
/// | Step | Bound | Default |
/// |---|---|---|
/// | the transports stop, together | each its own: the HTTP window then [`SHUTDOWN_SETTLE_TIMEOUT`], the Redis worker's drain window, the scheduler's tick bound then the settle | 20.5 s, the longest |
/// | the shutdown hooks run | this budget, across all three phases | 5 s |
/// | telemetry flushes | `nest_rs_opentelemetry`'s flush bound, every provider at once | 3 s |
/// | the runtime is torn down | what remains of this budget, under `#[nest_rs::main]` | nothing past it |
///
/// 28.5 s, under the kill: a process past its grace dies without a line.
///
/// **Once it is spent, every later hook still starts**, polled once: one that
/// finishes without waiting finishes, one that waits is abandoned at `warn`. A
/// hook that blocks its thread is past any timer's reach.
///
/// The scheduler reads it as the window of a tick still running at shutdown.
pub const SHUTDOWN_HOOKS_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an edge that stops its running work on the way down waits for that
/// work to unwind, once stopped ([`SHUTDOWN_HOOKS_TIMEOUT`] tabulates the way down).
///
/// Without it, a stopped unit may still be unwinding when the shutdown hooks start.
pub const SHUTDOWN_SETTLE_TIMEOUT: Duration = Duration::from_millis(500);

/// Lifecycle phase at which a hook runs. Init phases run before serving;
/// shutdown phases after the transports stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LifecyclePhase {
    /// First init phase — each module's own setup, before cross-cutting bootstrap.
    OnModuleInit,
    /// Second init phase — app-wide setup once every module has initialized.
    OnApplicationBootstrap,
    /// First shutdown phase — per-module teardown after transports stop.
    OnModuleDestroy,
    /// Shutdown notification, before the app-wide shutdown work runs.
    BeforeApplicationShutdown,
    /// Final shutdown phase — the last hooks to run before the process exits.
    OnApplicationShutdown,
}

type HookFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

pub(crate) use self::__private::LifecycleHook;

/// `lifecycle` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use std::any::TypeId;

    use super::{HookFuture, LifecyclePhase};
    use crate::container::Container;

    /// One lifecycle hook submitted to the link-time registry by `#[hooks]`.
    pub struct LifecycleHook {
        /// The phase this hook runs in.
        pub phase: LifecyclePhase,
        /// The host provider's name, the primary key of the run order.
        pub provider: &'static str,
        /// The hook method's name, the run order's tiebreaker.
        pub method: &'static str,
        /// `module_path!()` at the `#[hooks]` site, read by
        /// [`is_framework_owned`](crate::is_framework_owned).
        pub origin: &'static str,
        /// The host provider's type, read by [`inert_host`](crate::inert_host).
        pub provider_type_id: fn() -> TypeId,
        /// Whether this hook's provider is resolvable in the assembled container;
        /// a hook that self-gates inside `run` passes `|_| true`.
        pub present: fn(&Container) -> bool,
        /// Resolve the provider and invoke the hook method against the container.
        pub run: for<'a> fn(&'a Container) -> HookFuture<'a>,
    }
}

inventory::collect!(LifecycleHook);

fn hooks_for(phase: LifecyclePhase) -> Vec<&'static LifecycleHook> {
    let mut hooks: Vec<&'static LifecycleHook> = inventory::iter::<LifecycleHook>()
        .filter(|hook| hook.phase == phase)
        .collect();
    hooks.sort_by_key(|hook| (hook.provider, hook.method));
    hooks
}

/// Report a hook linked but whose host the booted container does not hold:
/// `warn` for the app's own code, `debug` for what is not the app's.
fn report_inert_hook(container: &Container, hook: &LifecycleHook, phase: LifecyclePhase) {
    crate::report_inert_host!(
        target: crate::target::LIFECYCLE,
        what: "lifecycle hook",
        origin: hook.origin,
        host: (hook.provider_type_id)(),
        container: container,
        phase = ::tracing::field::debug(phase),
        provider = hook.provider,
        method = hook.method,
    );
}

/// Init-phase runner: aborts on the first error or panic.
pub(crate) async fn run_phase(container: &Container, phase: LifecyclePhase) -> anyhow::Result<()> {
    for hook in hooks_for(phase) {
        if !(hook.present)(container) {
            report_inert_hook(container, hook, phase);
            continue;
        }
        tracing::debug!(
            target: crate::target::LIFECYCLE,
            ?phase,
            provider = hook.provider,
            method = hook.method,
            "running lifecycle hook",
        );
        match contained((hook.run)(container)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                return Err(err.context(format!(
                    "lifecycle hook {}::{} ({phase:?}) failed",
                    hook.provider, hook.method
                )));
            }
            Err(payload) => {
                crate::contained_panic!(
                    target: crate::target::LIFECYCLE,
                    payload.as_ref(),
                    "lifecycle hook panicked; the boot is aborted",
                    ?phase,
                    provider = hook.provider,
                    method = hook.method,
                    origin = hook.origin,
                );
                anyhow::bail!(
                    "lifecycle hook {}::{} ({phase:?}) panicked",
                    hook.provider,
                    hook.method,
                );
            }
        }
    }
    Ok(())
}

async fn contained(
    hook: HookFuture<'_>,
) -> Result<anyhow::Result<()>, Box<dyn std::any::Any + Send>> {
    AssertUnwindSafe(hook).catch_unwind().await
}

/// Shutdown-phase runner: logs a failure or panic and continues, bounded by
/// `deadline`, which the three shutdown phases share ([`SHUTDOWN_HOOKS_TIMEOUT`]).
pub(crate) async fn run_phase_lenient(
    container: &Container,
    phase: LifecyclePhase,
    deadline: Instant,
    way_down: &WayDown,
) {
    for hook in hooks_for(phase) {
        if !(hook.present)(container) {
            report_inert_hook(container, hook, phase);
            continue;
        }
        way_down.hook(phase, hook.provider, hook.method);
        let started = Instant::now();
        // `timeout_at` polls the hook before its timer, so past the deadline
        // it still runs once.
        match tokio::time::timeout_at(deadline, contained((hook.run)(container))).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => tracing::error!(
                target: crate::target::LIFECYCLE,
                ?phase,
                provider = hook.provider,
                method = hook.method,
                origin = hook.origin,
                error = %crate::error_message(&*err),
                "lifecycle hook failed",
            ),
            Ok(Err(payload)) => crate::contained_panic!(
                target: crate::target::LIFECYCLE,
                payload.as_ref(),
                "shutdown hook panicked, and the hooks after it still run",
                ?phase,
                provider = hook.provider,
                method = hook.method,
                origin = hook.origin,
            ),
            Err(_) => tracing::warn!(
                target: crate::target::LIFECYCLE,
                ?phase,
                provider = hook.provider,
                method = hook.method,
                origin = hook.origin,
                waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                budget_ms = u64::try_from(SHUTDOWN_HOOKS_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                "shutdown hook abandoned: the shutdown hooks' budget was spent while it waited, \
                 and the hooks after it still start",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Probe {
        hits: AtomicUsize,
    }

    impl Probe {
        async fn touch(&self) -> anyhow::Result<()> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // `#[hooks]` lives in `nest-rs-core-macros`, so this test hand-writes the thunk.
    fn run_touch(container: &Container) -> HookFuture<'_> {
        Box::pin(async move {
            match container.get::<Probe>() {
                Some(probe) => probe.touch().await,
                None => Ok(()),
            }
        })
    }

    inventory::submit! {
        LifecycleHook {
            phase: LifecyclePhase::OnModuleInit,
            provider: "Probe",
            method: "touch",
            origin: module_path!(),
            provider_type_id: TypeId::of::<Probe>,
            present: |container| container.get::<Probe>().is_some(),
            run: run_touch,
        }
    }

    #[tokio::test]
    async fn runs_registered_init_hook_against_the_container_instance() {
        let container = Container::builder()
            .provide(Probe {
                hits: AtomicUsize::new(0),
            })
            .build();
        run_phase(&container, LifecyclePhase::OnModuleInit)
            .await
            .unwrap();
        assert_eq!(
            container
                .get::<Probe>()
                .unwrap()
                .hits
                .load(Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn phase_with_no_hooks_is_a_noop() {
        let container = Container::builder().build();
        run_phase(&container, LifecyclePhase::OnApplicationShutdown)
            .await
            .unwrap();
    }

    struct Unreachable;

    fn run_unreachable(_container: &Container) -> HookFuture<'_> {
        Box::pin(async { panic!("an unreachable hook must be skipped, never run") })
    }

    inventory::submit! {
        LifecycleHook {
            phase: LifecyclePhase::BeforeApplicationShutdown,
            provider: "Unreachable",
            method: "never",
            origin: module_path!(),
            provider_type_id: TypeId::of::<Unreachable>,
            present: |_| false,
            run: run_unreachable,
        }
    }

    #[tokio::test]
    async fn an_unreachable_hook_is_skipped_by_both_runners() {
        let container = Container::builder().build();
        run_phase(&container, LifecyclePhase::BeforeApplicationShutdown)
            .await
            .expect("a skipped hook must not fail the phase");
        run_phase_lenient(
            &container,
            LifecyclePhase::BeforeApplicationShutdown,
            Instant::now() + SHUTDOWN_HOOKS_TIMEOUT,
            &WayDown::default(),
        )
        .await;
    }
}

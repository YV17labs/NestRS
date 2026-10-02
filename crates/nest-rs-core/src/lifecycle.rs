//! Application lifecycle hooks for the module/application init and shutdown
//! phases.
//!
//! A provider opts in by tagging methods on an impl block with `#[hooks]`. Each
//! hook is submitted to a link-time `inventory` registry that
//! [`crate::App::run`] drains per phase. Submitting to `inventory` lets a
//! provider keep its single `impl Discoverable` from `#[injectable]`.
//!
//! Ordering within a phase is `(provider, method)` name to be stable across
//! builds. Cross-provider init dependencies are not expressed here — a hook
//! that needs another service injects it.

use std::any::TypeId;
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
/// **One budget for the whole teardown, never one per hook**, because the budget
/// it is spent from is per process. Kubernetes gives a pod 30 seconds between
/// `SIGTERM` and `SIGKILL` by default, and the way down spends them in steps,
/// each bounded by default so they sum under the grace:
///
/// | Step | Bound | Default |
/// |---|---|---|
/// | the transports stop, together | each its own: the HTTP window then [`SHUTDOWN_SETTLE_TIMEOUT`], the Redis worker's drain window, the scheduler's tick bound then the settle | 20.5 s, the longest |
/// | the shutdown hooks run | this budget, across all three phases | 5 s |
/// | telemetry flushes | `nest_rs_opentelemetry`'s flush bound, every provider at once | 3 s |
/// | the runtime is torn down | what remains of this budget, under `#[nest_rs::main]` | nothing past it |
///
/// 28.5 seconds, one and a half short of the kill: a process past its grace dies
/// without a line, skipping every later hook and the flush. A bound per hook
/// could not hold that sum — `k` hooks that hang cost `k` times the bound — which
/// is why the budget is a deadline the three phases share. The runtime's own
/// teardown adds nothing to it: it is held to what the hooks and the flush left
/// of this deadline, so work they abandoned no longer holds the exit.
///
/// **Once it is spent, every later hook still starts.** Each is polled once
/// against the elapsed deadline: a hook that finishes without waiting — a
/// counter logged, a buffer handed to a channel — finishes, and one that waits
/// is abandoned at once, with the same `warn` naming it. A hook is never skipped
/// in silence.
///
/// A constant rather than a setting: a cleanup that needs longer is draining
/// work, and draining belongs in a transport's own window, which a deployment
/// does configure. The bound covers a hook that waits — an `async fn` pending on
/// I/O, a lock, a channel; one that blocks its thread is past any timer's reach,
/// and the process exit no longer waits for it either.
///
/// The scheduler reads it as its own window: a tick still running when shutdown
/// is asked for is developer code on the way down, as a hook is, and gets the
/// same time before it is stopped.
pub const SHUTDOWN_HOOKS_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an edge that stops its running work on the way down waits for that
/// work to unwind, once stopped.
///
/// Stopping drops each unit where it waits, so what is left is only for the
/// runtime to poll the tasks it woke and run what their drops do — the
/// `cancelled` line among them — which takes microseconds, unless a unit blocks
/// its thread, which no timer can reach. Waiting at all is what makes "nothing
/// stopped is still running when the shutdown hooks start" a fact rather than
/// a race; waiting longer would let a unit that blocks hold the stop.
///
/// One constant for every edge that stops work itself — the HTTP transport, for
/// what a self-mount runs off its connections, and the scheduler, for a tick
/// still running at its bound — so the way down is one arithmetic: half a
/// second after the edge's own window, inside the slack the default shutdown
/// leaves under a Kubernetes pod's grace ([`SHUTDOWN_HOOKS_TIMEOUT`] tabulates
/// it).
pub const SHUTDOWN_SETTLE_TIMEOUT: Duration = Duration::from_millis(500);

/// Lifecycle phase at which a hook runs. Init phases run after the container
/// is built and transports configured, before serving; shutdown phases run
/// after the transports stop.
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

/// One lifecycle hook submitted to the link-time registry by `#[hooks]`.
///
/// **Internal ABI** — macro-constructed, lockstep with `nest-rs-core`; do not
/// hand-construct.
#[doc(hidden)]
pub struct LifecycleHook {
    /// The phase this hook runs in.
    pub phase: LifecyclePhase,
    /// The host provider's name — the primary key of the `(provider, method)`
    /// run order and the label in the boot trace.
    pub provider: &'static str,
    /// The hook method's name — the tiebreaker in the `(provider, method)` order.
    pub method: &'static str,
    /// `module_path!()` at the `#[hooks]` site — the crate and module the
    /// provider lives in. Reported alongside the provider so "which of my
    /// modules owns this?" is answered by the log line, and used to decide
    /// whether an unreachable hook is the developer's problem
    /// ([`is_framework_owned`]).
    pub origin: &'static str,
    /// The host provider's type — what [`inert_host`](crate::inert_host) reads
    /// to say why a hook whose host is absent is inert.
    pub provider_type_id: fn() -> TypeId,
    /// Whether this hook's provider is resolvable in the assembled container.
    /// `#[hooks]` emits a `Container::get::<Provider>().is_some()` probe, so a
    /// hook whose provider was never listed in any reachable module is surfaced
    /// with a boot `warn` and skipped — leftover code stays visible instead of
    /// vanishing silently (the module-gated discovery rule). Module-level infra
    /// hooks that self-gate inside `run` pass `|_| true` to opt out.
    pub present: fn(&Container) -> bool,
    /// Resolve the provider and invoke the hook method against the container.
    pub run: for<'a> fn(&'a Container) -> HookFuture<'a>,
}

inventory::collect!(LifecycleHook);

fn hooks_for(phase: LifecyclePhase) -> Vec<&'static LifecycleHook> {
    let mut hooks: Vec<&'static LifecycleHook> = inventory::iter::<LifecycleHook>()
        .filter(|hook| hook.phase == phase)
        .collect();
    hooks.sort_by_key(|hook| (hook.provider, hook.method));
    hooks
}

/// Report an inert hook: linked, but the booted container holds no instance
/// under its host's own type, so it never fires.
///
/// **`warn` for the app's own code** — leftover code must stay visible instead
/// of vanishing silently (the module-gated discovery rule), and the developer
/// can act: the line names the cause and its remedy ([`InertHost`]).
///
/// **`debug` for what is not the app's** — the framework's capabilities it
/// never opted into, and another binary's hosts in a shared library crate. A
/// `warn` naming either teaches exactly one thing: that these warnings are
/// noise. Security warnings share this target.
///
/// [`InertHost`]: crate::InertHost
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

/// Init-phase runner: sequential, aborts on the first error.
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
                // The message rides the line, under the field every contained
                // panic is logged with; the error names the hook, as a failed
                // one's does, and aborts the boot the same way.
                tracing::error!(
                    target: crate::target::LIFECYCLE,
                    ?phase,
                    provider = hook.provider,
                    method = hook.method,
                    origin = hook.origin,
                    panic = crate::panic_message(payload.as_ref()),
                    "lifecycle hook panicked; the boot is aborted",
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

/// `hook`, with a panic inside it caught and handed back as its payload.
///
/// A hook is developer code, and an unwind out of one used to leave `App::run`
/// with it: at init, the boot aborted with no line naming the hook; at shutdown,
/// every later hook — in its phase and in the phases after — never ran. Both
/// runners contain it, and each reports it the way it reports an error.
async fn contained(
    hook: HookFuture<'_>,
) -> Result<anyhow::Result<()>, Box<dyn std::any::Any + Send>> {
    AssertUnwindSafe(hook).catch_unwind().await
}

/// Shutdown-phase runner: best-effort, logs failures and continues so one
/// provider's cleanup error — or panic — does not skip another's, and bounded by
/// `deadline`, which the three shutdown phases share
/// ([`SHUTDOWN_HOOKS_TIMEOUT`]), so neither does one provider's cleanup that
/// never returns. An abandoned hook's future is dropped where it waits, as any
/// cancelled task is: what it held is released, and what it had not yet done
/// stays undone, which is why the line naming it is a `warn`.
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
        // Polled once even past the deadline — `timeout_at` polls the hook
        // before its timer — so a hook that finishes without waiting still runs
        // once the budget is spent. See `SHUTDOWN_HOOKS_TIMEOUT`.
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
            Ok(Err(payload)) => tracing::error!(
                target: crate::target::LIFECYCLE,
                ?phase,
                provider = hook.provider,
                method = hook.method,
                origin = hook.origin,
                panic = crate::panic_message(payload.as_ref()),
                "shutdown hook panicked, and the hooks after it still run",
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

    // A hook whose `present` probe returns false models a `#[hooks]` provider
    // listed in no reachable module: it must be warned-and-skipped, never run.
    // `run_unreachable` panics if invoked, so a regression that drops the
    // `present` gate fails this test loudly.
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
        // Init runner: present=false ⇒ warn + skip, so the phase still succeeds
        // and `run_unreachable` never fires.
        run_phase(&container, LifecyclePhase::BeforeApplicationShutdown)
            .await
            .expect("a skipped hook must not fail the phase");
        // Shutdown runner: same skip, best-effort (also must not panic).
        run_phase_lenient(
            &container,
            LifecyclePhase::BeforeApplicationShutdown,
            Instant::now() + SHUTDOWN_HOOKS_TIMEOUT,
            &WayDown::default(),
        )
        .await;
    }
}

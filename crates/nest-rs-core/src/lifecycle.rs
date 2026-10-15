//! Lifecycle hooks: `#[hooks]` submits methods to a link-time `inventory`
//! registry that [`crate::App::run`] drains per phase, dependencies first at
//! init and in reverse at shutdown.

use std::any::TypeId;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::time::Instant;

use crate::access::positions;
use crate::app::BootStep;
use crate::app_lifecycle::AppLifecycle;
use crate::container::Container;
use crate::millis::millis;
use crate::panic;

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
/// | resources close, then the container drops | what the hooks left of this budget | inside the 5 s |
/// | telemetry flushes | `nest_rs_opentelemetry`'s flush bound, every provider at once | 3 s |
/// | the runtime is torn down | what remains of this budget, under `#[nest_rs::main]` | nothing past it |
/// | the console writes what it queues | `nest_rs_core::logging::CONSOLE_FLUSH_TIMEOUT`, under `#[nest_rs::main]` | 1 s |
///
/// 29.5 s, under the kill: a process past its grace dies without a line.
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

impl LifecyclePhase {
    fn shuts_down(self) -> bool {
        match self {
            Self::OnModuleInit | Self::OnApplicationBootstrap => false,
            Self::OnModuleDestroy
            | Self::BeforeApplicationShutdown
            | Self::OnApplicationShutdown => true,
        }
    }
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
    #[derive(Debug)]
    pub struct LifecycleHook {
        /// The phase this hook runs in.
        pub phase: LifecyclePhase,
        /// The host provider's name, for the lines naming the hook.
        pub provider: &'static str,
        /// The hook method's name, for the lines naming the hook.
        pub method: &'static str,
        /// The method's position among its `impl`'s hooks, the order a host's
        /// own hooks run in.
        pub declaration_index: usize,
        /// `module_path!()` at the `#[hooks]` site, read by
        /// [`inert_host`](crate::inert_host).
        pub origin: &'static str,
        /// The host provider's type, which ranks the hook in the run order
        /// and is read by [`inert_host`](crate::inert_host).
        pub provider_type_id: fn() -> TypeId,
        /// Whether this hook's provider is resolvable in the assembled container;
        /// a hook that self-gates inside `run` passes `|_| true`.
        pub present: fn(&Container) -> bool,
        /// Resolve the provider and invoke the hook method against the container.
        pub run: for<'a> fn(&'a Container) -> HookFuture<'a>,
    }
}

inventory::collect!(LifecycleHook);

/// Each provider's rank, by its own type, dependencies first: what `seal`
/// computes from the module descriptors, and the hook runners read.
pub(crate) struct HookOrder(HashMap<TypeId, usize>);

impl HookOrder {
    /// Rank each provider by its position in `order`.
    pub(crate) fn new(order: impl IntoIterator<Item = TypeId>) -> Self {
        Self(positions(order))
    }

    fn rank(&self, provider: TypeId) -> Option<usize> {
        self.0.get(&provider).copied()
    }
}

fn hooks_for(phase: LifecyclePhase, container: &Container) -> Vec<&'static LifecycleHook> {
    let mut hooks: Vec<&'static LifecycleHook> = inventory::iter::<LifecycleHook>()
        .filter(|hook| hook.phase == phase)
        .collect();
    sort_hooks(&mut hooks, phase, container.get::<HookOrder>().as_deref());
    hooks
}

/// Where a host runs among the others: one [`HookOrder`] does not rank goes
/// before every ranked one, and among such hosts by name, then the first module
/// its hooks sit in, then its type, so no two hosts' hooks interleave.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Host {
    Unranked {
        provider: &'static str,
        origin: &'static str,
        id: TypeId,
    },
    Ranked(usize),
}

/// Put `hooks` in `phase`'s run order: the hosts as [`Host`] orders them,
/// reversed in a shutdown phase; a host's own hooks by the module of their
/// `impl`, then as it declares them, then by method, in every phase.
fn sort_hooks(
    hooks: &mut [&'static LifecycleHook],
    phase: LifecyclePhase,
    order: Option<&HookOrder>,
) {
    let mut first_origin: HashMap<TypeId, &'static str> = HashMap::new();
    for hook in hooks.iter() {
        first_origin
            .entry((hook.provider_type_id)())
            .and_modify(|origin| *origin = (*origin).min(hook.origin))
            .or_insert(hook.origin);
    }
    let host = |hook: &LifecycleHook| {
        let id = (hook.provider_type_id)();
        match order.and_then(|order| order.rank(id)) {
            Some(rank) => Host::Ranked(rank),
            None => Host::Unranked {
                provider: hook.provider,
                origin: first_origin[&id],
                id,
            },
        }
    };
    let own = |hook: &LifecycleHook| (hook.origin, hook.declaration_index, hook.method);
    if phase.shuts_down() {
        hooks.sort_by_cached_key(|hook| (Reverse(host(hook)), own(hook)));
    } else {
        hooks.sort_by_cached_key(|hook| (host(hook), own(hook)));
    }
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

/// An init hook that failed, panicked or was interrupted: the hook, for the
/// line opening the teardown, and the error the boot returns.
#[derive(Debug)]
pub(crate) struct HookFailure {
    pub(crate) hook: &'static LifecycleHook,
    pub(crate) error: anyhow::Error,
}

/// Init-phase runner: aborts on the first error or panic, and at a stop asked
/// while a hook waits.
pub(crate) async fn run_phase(
    container: &Container,
    lifecycle: &AppLifecycle,
    phase: LifecyclePhase,
) -> Result<(), HookFailure> {
    for hook in hooks_for(phase, container) {
        if !(hook.present)(container) {
            report_inert_hook(container, hook, phase);
            continue;
        }
        tracing::debug!(
            target: crate::target::LIFECYCLE,
            ?phase,
            provider = hook.provider,
            method = hook.method,
            origin = hook.origin,
            "running lifecycle hook",
        );
        let step = BootStep::InitHook {
            phase,
            provider: hook.provider,
            method: hook.method,
            origin: hook.origin,
        };
        let error = match lifecycle
            .boot(&step, panic::contain((hook.run)(container)))
            .await
        {
            Ok(Ok(Ok(()))) => continue,
            Err(interrupted) => interrupted.into(),
            Ok(Ok(Err(err))) => err.context(format!("{step} failed")),
            Ok(Err(payload)) => {
                crate::contained_panic!(
                    target: crate::target::LIFECYCLE,
                    payload.as_ref(),
                    "lifecycle hook panicked; the boot is aborted",
                    ?phase,
                    provider = hook.provider,
                    method = hook.method,
                    origin = hook.origin,
                );
                anyhow::anyhow!("{step} panicked")
            }
        };
        return Err(HookFailure { hook, error });
    }
    Ok(())
}

/// Shutdown-phase runner: logs a failure or panic and continues, bounded by
/// `deadline`, which the three shutdown phases share ([`SHUTDOWN_HOOKS_TIMEOUT`]).
pub(crate) async fn run_phase_lenient(
    container: &Container,
    phase: LifecyclePhase,
    deadline: Instant,
) {
    for hook in hooks_for(phase, container) {
        if !(hook.present)(container) {
            report_inert_hook(container, hook, phase);
            continue;
        }
        tracing::debug!(
            target: crate::target::LIFECYCLE,
            ?phase,
            provider = hook.provider,
            method = hook.method,
            origin = hook.origin,
            "running lifecycle hook",
        );
        crate::way_down::hook(phase, hook.provider, hook.method, hook.origin);
        let started = Instant::now();
        // `timeout_at` polls the hook before its timer, so past the deadline
        // it still runs once.
        match tokio::time::timeout_at(deadline, panic::contain((hook.run)(container))).await {
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
                waited_ms = millis(started.elapsed()),
                budget_ms = millis(SHUTDOWN_HOOKS_TIMEOUT),
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
            declaration_index: 0,
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
        run_phase(
            &container,
            &AppLifecycle::new(),
            LifecyclePhase::OnModuleInit,
        )
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
        run_phase(
            &container,
            &AppLifecycle::new(),
            LifecyclePhase::OnApplicationShutdown,
        )
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
            declaration_index: 0,
            origin: module_path!(),
            provider_type_id: TypeId::of::<Unreachable>,
            present: |_| false,
            run: run_unreachable,
        }
    }

    struct Described;
    struct Seeded;

    fn noop(_container: &Container) -> HookFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    fn hook(
        provider: &'static str,
        method: &'static str,
        declaration_index: usize,
        provider_type_id: fn() -> TypeId,
    ) -> &'static LifecycleHook {
        hook_in(
            module_path!(),
            provider,
            method,
            declaration_index,
            provider_type_id,
        )
    }

    fn hook_in(
        origin: &'static str,
        provider: &'static str,
        method: &'static str,
        declaration_index: usize,
        provider_type_id: fn() -> TypeId,
    ) -> &'static LifecycleHook {
        Box::leak(Box::new(LifecycleHook {
            phase: LifecyclePhase::OnModuleInit,
            provider,
            method,
            declaration_index,
            origin,
            provider_type_id,
            present: |_| true,
            run: noop,
        }))
    }

    fn ran(hooks: &[&'static LifecycleHook]) -> Vec<&'static str> {
        hooks.iter().map(|hook| hook.method).collect()
    }

    #[test]
    fn a_host_no_descriptor_ranks_runs_first_at_init_and_last_at_shutdown() {
        let order = HookOrder::new([TypeId::of::<Described>()]);
        let mut hooks = vec![
            hook(
                "Adescribed",
                "zz_first_declared",
                0,
                TypeId::of::<Described>,
            ),
            hook(
                "Adescribed",
                "aa_second_declared",
                1,
                TypeId::of::<Described>,
            ),
            hook("Zseeded", "seeded", 0, TypeId::of::<Seeded>),
        ];
        sort_hooks(&mut hooks, LifecyclePhase::OnModuleInit, Some(&order));
        assert_eq!(
            ran(&hooks),
            ["seeded", "zz_first_declared", "aa_second_declared"]
        );
        sort_hooks(&mut hooks, LifecyclePhase::OnModuleDestroy, Some(&order));
        assert_eq!(
            ran(&hooks),
            ["zz_first_declared", "aa_second_declared", "seeded"],
            "the hosts in reverse, each host's own hooks as its impl declares them",
        );
    }

    struct Pool;
    struct SeededTwin;

    #[test]
    fn a_host_whose_hooks_span_several_impls_keeps_them_together_in_one_order_both_ways() {
        let order = HookOrder::new([TypeId::of::<Described>(), TypeId::of::<Pool>()]);
        let mut hooks = vec![
            hook_in("app::b", "Pool", "open", 0, TypeId::of::<Pool>),
            hook_in("app::b", "Pool", "close", 1, TypeId::of::<Pool>),
            hook_in("app::z", "Cache", "z_seeded", 0, TypeId::of::<Seeded>),
            hook_in("app::a", "Pool", "migrate", 0, TypeId::of::<Pool>),
            hook_in("app::m", "Cache", "m_twin", 0, TypeId::of::<SeededTwin>),
            hook_in("app::a", "Pool", "report", 1, TypeId::of::<Pool>),
            hook_in("app", "Pool", "warm", 0, TypeId::of::<Pool>),
            hook_in("app::a", "Cache", "a_seeded", 0, TypeId::of::<Seeded>),
            hook_in("app", "Pool", "check", 0, TypeId::of::<Pool>),
            hook_in(
                "app::x",
                "Described",
                "described",
                0,
                TypeId::of::<Described>,
            ),
        ];
        let pool = ["check", "warm", "migrate", "report", "open", "close"];
        sort_hooks(&mut hooks, LifecyclePhase::OnModuleInit, Some(&order));
        assert_eq!(
            ran(&hooks),
            [&["a_seeded", "z_seeded", "m_twin", "described"][..], &pool].concat(),
            "a host's impls by module path, each impl's hooks as declared, ties by method name; \
             unlisted hosts by name, then the first module their hooks sit in",
        );
        sort_hooks(&mut hooks, LifecyclePhase::OnModuleDestroy, Some(&order));
        assert_eq!(
            ran(&hooks),
            [&pool[..], &["described", "m_twin", "a_seeded", "z_seeded"]].concat(),
            "the hosts in exact reverse, each host's own hooks in the same order as at init",
        );
    }

    #[tokio::test]
    async fn an_unreachable_hook_is_skipped_by_both_runners() {
        let container = Container::builder().build();
        run_phase(
            &container,
            &AppLifecycle::new(),
            LifecyclePhase::BeforeApplicationShutdown,
        )
        .await
        .expect("a skipped hook must not fail the phase");
        run_phase_lenient(
            &container,
            LifecyclePhase::BeforeApplicationShutdown,
            Instant::now() + SHUTDOWN_HOOKS_TIMEOUT,
        )
        .await;
    }
}

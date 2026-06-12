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

use std::future::Future;
use std::pin::Pin;

use crate::container::Container;

/// Lifecycle phase at which a hook runs. Init phases run after the container
/// is built and transports configured, before serving; shutdown phases run
/// after the transports stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecyclePhase {
    OnModuleInit,
    OnApplicationBootstrap,
    OnModuleDestroy,
    BeforeApplicationShutdown,
    OnApplicationShutdown,
}

type HookFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// One lifecycle hook submitted to the link-time registry by `#[hooks]`.
pub struct LifecycleHook {
    pub phase: LifecyclePhase,
    pub provider: &'static str,
    pub method: &'static str,
    /// Whether this hook's provider is resolvable in the assembled container.
    /// `#[hooks]` emits a `Container::get::<Provider>().is_some()` probe, so a
    /// hook whose provider was never listed in any reachable module is surfaced
    /// with a boot `warn` and skipped — leftover code stays visible instead of
    /// vanishing silently (the module-gated discovery rule). Module-level infra
    /// hooks that self-gate inside `run` pass `|_| true` to opt out.
    pub present: fn(&Container) -> bool,
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

/// Emit the inert-hook `warn`: a linked hook whose provider is unreachable from
/// the running app's module tree never fires, so surface it instead of letting
/// it disappear (mirrors the schedule/queue/resolver discovery warns).
fn warn_unreachable_hook(hook: &LifecycleHook, phase: LifecyclePhase) {
    tracing::warn!(
        target: "nest_rs::lifecycle",
        ?phase,
        provider = hook.provider,
        method = hook.method,
        "skipped lifecycle hook: provider unreachable from app's module tree",
    );
}

/// Init-phase runner: sequential, aborts on the first error.
pub(crate) async fn run_phase(container: &Container, phase: LifecyclePhase) -> anyhow::Result<()> {
    for hook in hooks_for(phase) {
        if !(hook.present)(container) {
            warn_unreachable_hook(hook, phase);
            continue;
        }
        tracing::debug!(
            target: "nest_rs::lifecycle",
            ?phase,
            provider = hook.provider,
            method = hook.method,
            "running lifecycle hook",
        );
        (hook.run)(container).await.map_err(|err| {
            err.context(format!(
                "lifecycle hook {}::{} ({phase:?}) failed",
                hook.provider, hook.method
            ))
        })?;
    }
    Ok(())
}

/// Shutdown-phase runner: best-effort, logs failures and continues so one
/// provider's cleanup error does not skip another's.
pub(crate) async fn run_phase_lenient(container: &Container, phase: LifecyclePhase) {
    for hook in hooks_for(phase) {
        if !(hook.present)(container) {
            warn_unreachable_hook(hook, phase);
            continue;
        }
        if let Err(err) = (hook.run)(container).await {
            tracing::error!(
                target: "nest_rs::lifecycle",
                ?phase,
                provider = hook.provider,
                method = hook.method,
                error = %err,
                "lifecycle hook failed",
            );
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

    // `#[hooks]` lives in `nestrs-macros`, so this test hand-writes the thunk.
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
    fn run_unreachable(_container: &Container) -> HookFuture<'_> {
        Box::pin(async { panic!("an unreachable hook must be skipped, never run") })
    }

    inventory::submit! {
        LifecycleHook {
            phase: LifecyclePhase::BeforeApplicationShutdown,
            provider: "Unreachable",
            method: "never",
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
        run_phase_lenient(&container, LifecyclePhase::BeforeApplicationShutdown).await;
    }
}

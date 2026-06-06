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

/// One lifecycle hook submitted to the link-time registry by `#[hooks]`. The
/// `run` thunk resolves the provider from the container; it is a no-op if the
/// provider was never registered in any module.
pub struct LifecycleHook {
    pub phase: LifecyclePhase,
    pub provider: &'static str,
    pub method: &'static str,
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

/// Init-phase runner: sequential, aborts on the first error.
pub(crate) async fn run_phase(container: &Container, phase: LifecyclePhase) -> anyhow::Result<()> {
    for hook in hooks_for(phase) {
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
}

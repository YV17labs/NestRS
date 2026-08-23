use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::{
    Container, ContainerBuilder, LifecycleHook, LifecyclePhase, Module, ProviderOrder,
    ReachableProviders, inventory,
};

use crate::{EventBus, ListenerMethod};

/// Registers the [`EventBus`] and wires every discovered `#[on_event]` method
/// at application bootstrap against the fully-assembled container.
pub struct EventsModule;

impl Module for EventsModule {
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(std::any::TypeId::of::<Self>()) {
            return builder;
        }
        builder.provide_arc(Arc::new(EventBus::new()))
    }
}

// No-op when EventsModule was not imported (the bus is then absent). Infra
// hook self-gates inside `wire_listeners`, so it opts out of the inert-hook
// warn with `present: |_| true`.
nest_rs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "EventsModule",
        method: "wire_listeners",
        origin: module_path!(),
        present: |_| true,
        run: wire_listeners,
    }
}

/// The sentence the boot files for a listener whose app registered no bus.
///
/// A constant rather than a literal because the test that proves this branch has
/// to name the same event — "assert against shared constants, never a copied
/// literal", so the report and the suite cannot drift apart.
pub const NO_BUS_REPORT: &str = "listener declared but no event bus is registered — add `EventsModule` to the root \
     module's `imports = [...]`";

fn wire_listeners(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        let reachable = container.get::<ReachableProviders>();
        let Some(bus) = container.get::<EventBus>() else {
            // Reachable listeners and no bus: every `#[on_event]` method the app
            // declared is dead, and nothing else can see it. Nothing in the
            // `#[listeners]` expansion makes a host depend on `EventBus`, so a
            // provider listed in `providers = [...]` without `EventsModule` in
            // `imports` boots clean and reacts to nothing — the reachable-set arm
            // below never fires, because the provider *is* reachable.
            //
            // This is the earliest site that can see the fact, so it owes the
            // report: a silent `Ok(())` here is a feature's whole reaction
            // surface disappearing with no trace.
            for entry in reachable_listeners(reachable.as_deref()) {
                tracing::warn!(
                    target: crate::TARGET,
                    listener = entry.name,
                    origin = entry.origin,
                    "{NO_BUS_REPORT}",
                );
            }
            return Ok(());
        };
        let order = container.get::<ProviderOrder>();

        // Sort before wiring. `inventory::iter` yields **link order** — stable
        // for a given binary, and reshuffled by any change to the code, so a
        // pair of listeners ordered deliberately and verified locally was
        // silently rearranged the next time somebody added a third to the same
        // block. The bus faithfully preserves whatever it is handed, so the
        // order has to be right here: providers as they appear in
        // `providers = [...]` (their rank in the module walk), then methods as
        // they appear in the `#[listeners]` block. `name` breaks the remaining
        // tie so an app booted without the access graph (a hand-built
        // container in a test) is deterministic rather than link-ordered.
        let mut entries: Vec<&'static ListenerMethod> =
            inventory::iter::<ListenerMethod>().collect();
        entries.sort_by_cached_key(|entry| {
            (
                order
                    .as_ref()
                    .map_or(0, |o| o.rank((entry.provider_type_id)())),
                entry.declaration_index,
                entry.name,
            )
        });

        for entry in entries {
            if !is_reachable(reachable.as_deref(), entry) {
                ::nest_rs_core::report_inert_host!(
                    target: crate::TARGET,
                    what: "#[on_event] method",
                    origin: entry.origin,
                    listener = entry.name,
                );
                continue;
            }
            (entry.wire)(container, &bus);
            tracing::debug!(
                target: crate::TARGET,
                listener = entry.name,
                "wired event listener",
            );
        }
        Ok(())
    })
}

/// Whether this app would wire `entry` at all — the one spelling of the
/// reachable-set gate, read by both the no-bus report and the wiring loop.
///
/// One predicate rather than two, because the report speaks about exactly the
/// population the loop wires: two spellings would let a change to the
/// reachability rule land in one of them and leave the report describing a set
/// the app does not have.
fn is_reachable(reachable: Option<&ReachableProviders>, entry: &ListenerMethod) -> bool {
    ReachableProviders::reaches(reachable, (entry.provider_type_id)())
}

/// The listeners this app would wire, in no particular order — the population
/// the no-bus report speaks about.
fn reachable_listeners(
    reachable: Option<&ReachableProviders>,
) -> impl Iterator<Item = &'static ListenerMethod> + '_ {
    inventory::iter::<ListenerMethod>().filter(move |entry| is_reachable(reachable, entry))
}

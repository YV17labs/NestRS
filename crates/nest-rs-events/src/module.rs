use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::__private::LifecycleHook;
use nest_rs_core::{
    Container, ContainerBuilder, LifecyclePhase, Module, ProviderOrder, ReachableProviders,
    Registering, inventory,
};

use crate::{EventBus, ListenerMethod};

/// Registers the [`EventBus`] and subscribes every reachable `#[on_event]`
/// method to it at the kernel's wiring step, from the assembled container and
/// before the first lifecycle hook — so an event an init hook emits reaches its
/// listeners.
pub struct EventsModule;

impl Module for EventsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_arc(Arc::new(EventBus::new()))
            .provide_wiring("nest_rs::events::listeners", wire_listeners)
    }
}

// The report fires exactly when `EventsModule` is absent, so no wiring of its
// can carry it: this hook is ungated (`present: |_| true`) and wires nothing.
nest_rs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "EventsModule",
        method: "report_missing_bus",
        origin: module_path!(),
        provider_type_id: std::any::TypeId::of::<EventsModule>,
        present: |_| true,
        run: report_missing_bus,
    }
}

/// The sentence the boot files for a listener whose app registered no bus.
pub const NO_BUS_REPORT: &str = "listener declared but no event bus is registered — add `EventsModule` to the root \
     module's `imports = [...]`";

/// The only site that sees reachable listeners without a bus: no host depends
/// on `EventBus`, so the app otherwise boots clean.
fn report_missing_bus(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        if container.get::<EventBus>().is_none() {
            let reachable = container.get::<ReachableProviders>();
            for entry in reachable_listeners(reachable.as_deref()) {
                tracing::warn!(
                    target: crate::TARGET,
                    listener = entry.name,
                    origin = entry.origin,
                    "{NO_BUS_REPORT}",
                );
            }
        }
        Ok(())
    })
}

/// Subscribe every reachable listener, in the written order, then open the bus.
fn wire_listeners(container: &Container) -> anyhow::Result<()> {
    let Some(bus) = container.get::<EventBus>() else {
        return Ok(());
    };
    let reachable = container.get::<ReachableProviders>();
    let order = container.get::<ProviderOrder>();

    // `inventory::iter` yields link order, reshuffled by any code change;
    // the bus keeps what it is handed, so the written order is restored here.
    let mut entries: Vec<&'static ListenerMethod> = inventory::iter::<ListenerMethod>().collect();
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
                host: (entry.provider_type_id)(),
                container: container,
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
    bus.mark_wired();
    Ok(())
}

/// Whether this app would wire `entry` — shared by the no-bus report and the
/// wiring loop so both speak of one population.
fn is_reachable(reachable: Option<&ReachableProviders>, entry: &ListenerMethod) -> bool {
    ReachableProviders::reaches(reachable, (entry.provider_type_id)())
}

fn reachable_listeners(
    reachable: Option<&ReachableProviders>,
) -> impl Iterator<Item = &'static ListenerMethod> + '_ {
    inventory::iter::<ListenerMethod>().filter(move |entry| is_reachable(reachable, entry))
}

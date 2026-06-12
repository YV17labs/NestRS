use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::{
    Container, ContainerBuilder, LifecycleHook, LifecyclePhase, Module, ReachableProviders,
    inventory,
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
        present: |_| true,
        run: wire_listeners,
    }
}

fn wire_listeners(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        let Some(bus) = container.get::<EventBus>() else {
            return Ok(());
        };
        let reachable = container.get::<ReachableProviders>();
        for entry in inventory::iter::<ListenerMethod>() {
            let provider_id = (entry.provider_type_id)();
            if let Some(r) = reachable.as_ref()
                && !r.0.contains(&provider_id)
            {
                tracing::warn!(
                    target: "nest_rs::events",
                    listener = entry.name,
                    "skipped #[on_event] method: provider unreachable from app's module tree",
                );
                continue;
            }
            (entry.wire)(container, &bus);
            tracing::debug!(
                target: "nest_rs::events",
                listener = entry.name,
                "wired event listener",
            );
        }
        Ok(())
    })
}

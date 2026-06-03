use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nestrs_core::{
    Container, ContainerBuilder, DiscoveryService, LifecycleHook, LifecyclePhase, Module,
};

use crate::{EventBus, EventHandlerMeta};

/// Registers the [`EventBus`] and wires every discovered `#[on_event]` at
/// application bootstrap against the fully-assembled container.
pub struct EventModule;

impl Module for EventModule {
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(std::any::TypeId::of::<Self>()) {
            return builder;
        }
        builder.provide_arc(Arc::new(EventBus::new()))
    }
}

// No-op when EventModule was not imported (the bus is then absent).
nestrs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "EventModule",
        method: "wire_handlers",
        run: wire_handlers,
    }
}

fn wire_handlers(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        let Some(bus) = container.get::<EventBus>() else {
            return Ok(());
        };
        let discovery = DiscoveryService::new(container);
        for handler in discovery.meta::<EventHandlerMeta>() {
            (handler.meta.wire)(container, &bus);
            tracing::debug!(
                target: "nestrs::events",
                handler = handler.meta.name,
                "wired event handler",
            );
        }
        Ok(())
    })
}

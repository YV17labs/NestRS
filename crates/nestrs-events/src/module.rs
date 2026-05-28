//! [`EventModule`] — registers the [`EventBus`] and wires discovered handlers.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nestrs_core::{
    Container, ContainerBuilder, DiscoveryService, LifecycleHook, LifecyclePhase, Module,
};

use crate::{EventBus, EventHandlerMeta};

/// Import it (`#[module(imports = [EventModule, ...])]`) to enable the event bus.
///
/// Registers the [`EventBus`] as a provider — inject `Arc<EventBus>` to
/// [`emit`](EventBus::emit) — and, at application bootstrap, wires every
/// discovered `#[event_handler]` into it. Wiring runs against the fully-assembled
/// container, so a handler may inject any provider regardless of module import
/// order, mirroring how the `Scheduler` transport reads its jobs.
pub struct EventModule;

impl Module for EventModule {
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        // Idempotent like a macro-generated module: a diamond import registers once.
        if !builder.mark_registered(std::any::TypeId::of::<Self>()) {
            return builder;
        }
        builder.provide_arc(Arc::new(EventBus::new()))
    }
}

// Wire handlers at bootstrap, when the container is complete. Submitted to the
// same link-time registry `#[hooks]` uses; `App::run` / `App::init` drains it. A
// no-op if `EventModule` was not imported (the bus is then absent).
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
            return Ok(()); // EventModule not imported — nothing to wire.
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

//! Request-scoped DataLoaders, discovered at link time.
//!
//! `#[dataloader]` generates one batching loader per method and submits a
//! [`LoaderRegistration`]. The loader is rebuilt per request and seeded into
//! the GraphQL context by [`LoaderExtension`], where a `#[field]` reads it as
//! `&DataLoader<…>`. Per-request build makes module import order irrelevant:
//! the container is fully assembled when the request arrives.

use std::any::TypeId;
use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest,
};
use async_graphql::{Request, ServerResult};
use nestrs_core::{Container, ReachableProviders};

/// One DataLoader registration. `owner_type_id` is the `TypeId` of the
/// `#[dataloader]` impl's `Self`; when the owner is not in
/// [`ReachableProviders`], `container.get::<Self>()` would panic at request
/// time, so the seed is module-gated by the owner's reachability.
#[doc(hidden)]
pub struct LoaderRegistration {
    pub owner_type_id: fn() -> TypeId,
    pub seed: fn(&Container, Request) -> Request,
}

inventory::collect!(LoaderRegistration);

pub type BatchFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

pub type BatchSpawner = Box<dyn Fn(BatchFuture) + Send + Sync>;

/// Re-establishes per-request ambient state inside a DataLoader batch.
/// async-graphql runs every batch on a spawned task, which starts with empty
/// task-local storage — so the ambient executor + ability a request installs
/// are gone by the time a batch loads, and a loader's `Repo` reads would run
/// unscoped. `spawner` is called per request inside the operation's ambient
/// scope so the implementor can snapshot that state into the returned
/// spawner.
///
/// Bind with `providers = [MyBridge as dyn BatchContext]`. With none
/// registered, batches spawn bare on `tokio::spawn` (correct for an app
/// without row-level security).
pub trait BatchContext: Send + Sync + 'static {
    fn spawner(&self) -> BatchSpawner;
}

#[doc(hidden)]
pub fn batch_spawner(container: &Container) -> BatchSpawner {
    match container.get_dyn::<dyn BatchContext>() {
        Some(ctx) => ctx.spawner(),
        None => Box::new(|fut| {
            tokio::spawn(fut);
        }),
    }
}

/// Seeds every discovered DataLoader into each GraphQL request.
pub(crate) struct LoaderExtensionFactory {
    container: Container,
}

impl LoaderExtensionFactory {
    pub(crate) fn new(container: Container) -> Self {
        Self { container }
    }
}

impl ExtensionFactory for LoaderExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(LoaderExtension {
            container: self.container.clone(),
        })
    }
}

struct LoaderExtension {
    container: Container,
}

#[async_trait]
impl Extension for LoaderExtension {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        mut request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        // Module-gate: a loader from an unimported module would panic on
        // `container.get::<Owner>()`. Fail closed when the gate is missing —
        // a hand-rolled container is the only way for `ReachableProviders`
        // to be unseeded, and we prefer skipping every loader to panicking
        // on the first request that touches one.
        let Some(reachable) = self.container.get::<ReachableProviders>() else {
            tracing::warn!(
                target: "nestrs::graphql",
                "LoaderExtension: no ReachableProviders seeded — skipping every loader. \
                 Build the schema through App::builder/App::new (production) or seed \
                 the marker on the hand-rolled container."
            );
            return next.run(ctx, request).await;
        };
        for reg in inventory::iter::<LoaderRegistration>() {
            if !reachable.0.contains(&(reg.owner_type_id)()) {
                continue;
            }
            request = (reg.seed)(&self.container, request);
        }
        next.run(ctx, request).await
    }
}

//! Request-scoped DataLoaders, discovered at link time.
//!
//! `#[dataloader]` generates one batching loader per method and submits a
//! [`LoaderRegistration`]. The loader is rebuilt per request and seeded into
//! the GraphQL context by [`LoaderExtension`], where a `#[field_resolver]` reads it as
//! `&DataLoader<…>`. Per-request build makes module import order irrelevant:
//! the container is fully assembled when the request arrives.

use std::any::TypeId;
use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest,
};
use async_graphql::{Request, ServerResult};
use nest_rs_core::{Container, ReachableProviders};

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
                target: "nest_rs::graphql",
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    // Falling back to bare `tokio::spawn` is the documented "no row-level
    // security" path. Pin that the spawner actually runs the future end-to-end
    // when no `BatchContext` provider is registered.
    #[tokio::test]
    async fn batch_spawner_without_a_context_runs_the_future_on_tokio_spawn() {
        let container = Container::builder().build();
        let spawner = batch_spawner(&container);
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        spawner(Box::pin(async move {
            r.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(());
        }));
        rx.await.expect("spawned future resolves");
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    // A registered `BatchContext` provider must take over from the default
    // spawner. The trait is intentionally minimal so a bridge can install
    // ambient state around the future; verifying the dispatch (not just the
    // shape) is the regression check that matters.
    struct CountingContext {
        count: Arc<AtomicUsize>,
    }

    impl BatchContext for CountingContext {
        fn spawner(&self) -> BatchSpawner {
            let count = self.count.clone();
            Box::new(move |fut| {
                count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(fut);
            })
        }
    }

    #[tokio::test]
    async fn batch_spawner_routes_through_a_registered_batch_context() {
        let count = Arc::new(AtomicUsize::new(0));
        let ctx: Arc<dyn BatchContext> = Arc::new(CountingContext {
            count: count.clone(),
        });
        let container = Container::builder().provide_dyn(ctx).build();

        let spawner = batch_spawner(&container);
        let (tx, rx) = tokio::sync::oneshot::channel();
        spawner(Box::pin(async move {
            let _ = tx.send(());
        }));
        rx.await.expect("spawned future resolves");
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "the bridge's spawner must wrap the future, not be bypassed",
        );
    }
}

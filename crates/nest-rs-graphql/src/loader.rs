//! Request-scoped DataLoaders, discovered at link time.
//!
//! `#[dataloader]` generates one batching loader per method and submits a
//! [`GraphqlLoaderRegistration`]. The loader is rebuilt per request and seeded into
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
pub struct GraphqlLoaderRegistration {
    pub owner_type_id: fn() -> TypeId,
    pub seed: fn(&Container, Request) -> Request,
}

inventory::collect!(GraphqlLoaderRegistration);

/// A DataLoader batch's work, boxed for spawning on its own task.
pub type GraphqlBatchFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

/// Spawns a batch future, having re-established the request's ambient state
/// around it (see [`GraphqlBatchContext`]).
pub type GraphqlBatchSpawner = Box<dyn Fn(GraphqlBatchFuture) + Send + Sync>;

/// Re-establishes per-request ambient state inside a DataLoader batch.
/// async-graphql runs every batch on a spawned task, which starts with empty
/// task-local storage — so the ambient executor + ability a request installs
/// are gone by the time a batch loads, and a loader's `Repo` reads would run
/// unscoped. `spawner` is called per request inside the operation's ambient
/// scope so the implementor can snapshot that state into the returned
/// spawner.
///
/// Bind with `providers = [MyBridge as dyn GraphqlBatchContext]`. With none
/// registered, batches spawn bare on `tokio::spawn` (correct for an app
/// without row-level security).
pub trait GraphqlBatchContext: Send + Sync + 'static {
    /// Build a spawner that carries the current request's ambient executor +
    /// ability into each batch it runs.
    fn spawner(&self) -> GraphqlBatchSpawner;
}

#[doc(hidden)]
pub fn batch_spawner(container: &Container) -> GraphqlBatchSpawner {
    match container.get_dyn::<dyn GraphqlBatchContext>() {
        Some(ctx) => ctx.spawner(),
        None => Box::new(|fut| {
            tokio::spawn(fut);
        }),
    }
}

/// Seeds every discovered DataLoader into each GraphQL request.
/// The per-request seeding step of one reachable dataloader.
type LoaderSeed = fn(&Container, Request) -> Request;

pub(crate) struct LoaderExtensionFactory {
    container: Container,
    /// The reachable loaders, resolved **once** at schema build. Reachability
    /// is decided by the access graph and frozen from then on, so re-walking
    /// link-time inventory and re-testing the gate per request was pure
    /// repetition of a boot-time answer.
    seeds: Arc<[LoaderSeed]>,
}

impl LoaderExtensionFactory {
    pub(crate) fn new(container: Container) -> Self {
        warn_unreachable_loaders(&container);
        let seeds = reachable_seeds(&container);
        Self { container, seeds }
    }
}

/// Freeze the reachable loader seeds. A missing `ReachableProviders` (only a
/// hand-rolled container can produce one) seeds nothing: a loader whose owner
/// module is absent would panic on `container.get::<Owner>()`, so skipping is
/// the fail-closed answer — and it is reported here, once, rather than on
/// every request.
fn reachable_seeds(container: &Container) -> Arc<[LoaderSeed]> {
    let Some(reachable) = container.get::<ReachableProviders>() else {
        tracing::warn!(
            target: "nest_rs::graphql",
            hint = "build the schema via App::builder/App::new or seed ReachableProviders",
            "loaders skipped: no ReachableProviders seeded"
        );
        return Arc::from(Vec::new());
    };
    inventory::iter::<GraphqlLoaderRegistration>()
        .filter(|reg| reachable.0.contains(&(reg.owner_type_id)()))
        .map(|reg| reg.seed)
        .collect()
}

/// Boot-time visibility for the "linked but unreachable" case: a
/// `#[dataloader]` links into the binary but its owner service's module is not
/// imported by (or reachable from) this app's root. Such a loader is skipped
/// per request in `LoaderExtension::prepare_request` — seeding it would panic
/// on `container.get::<Owner>()` — so the skip must not be silent, per the
/// "linked but unreachable ⇒ boot `tracing::warn`" norm. Emitted once, at
/// schema build (`LoaderExtensionFactory::new`), not per request.
///
/// [`GraphqlLoaderRegistration`] carries only the owner's `TypeId`, not a name,
/// so this reports the count; the per-relation name lands in the query-time
/// resolver error (`nest-rs-resource-macros` emits `data_opt` + a named error,
/// not the panicking `data_unchecked`).
fn warn_unreachable_loaders(container: &Container) {
    // No gate seeded (a hand-rolled container in a test): `reachable_seeds`
    // warns once and seeds nothing — nothing to add here.
    let Some(reachable) = container.get::<ReachableProviders>() else {
        return;
    };
    let skipped = inventory::iter::<GraphqlLoaderRegistration>()
        .filter(|reg| !reachable.0.contains(&(reg.owner_type_id)()))
        .count();
    if skipped > 0 {
        tracing::warn!(
            target: "nest_rs::graphql",
            count = skipped,
            hint = "import the modules that provide these loaders; relation fields backed by them error at query time",
            "dataloaders linked but unreachable",
        );
    }
}

impl ExtensionFactory for LoaderExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(LoaderExtension {
            container: self.container.clone(),
            seeds: Arc::clone(&self.seeds),
        })
    }
}

struct LoaderExtension {
    container: Container,
    seeds: Arc<[LoaderSeed]>,
}

#[async_trait]
impl Extension for LoaderExtension {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        mut request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        for seed in self.seeds.iter() {
            request = seed(&self.container, request);
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
    // when no `GraphqlBatchContext` provider is registered.
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

    // A registered `GraphqlBatchContext` provider must take over from the default
    // spawner. The trait is intentionally minimal so a bridge can install
    // ambient state around the future; verifying the dispatch (not just the
    // shape) is the regression check that matters.
    struct CountingContext {
        count: Arc<AtomicUsize>,
    }

    impl GraphqlBatchContext for CountingContext {
        fn spawner(&self) -> GraphqlBatchSpawner {
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
        let ctx: Arc<dyn GraphqlBatchContext> = Arc::new(CountingContext {
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

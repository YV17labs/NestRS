//! Request-scoped DataLoaders, discovered at link time.
//!
//! `#[dataloader]` generates one batching loader per method and submits a
//! [`GraphqlLoaderRegistration`]; [`LoaderExtension`] rebuilds it per request
//! and seeds it into the GraphQL context for a `#[field_resolver]` to read.
//!
//! async-graphql runs every batch on a task of its own, so batches run in the
//! `/graphql` mount's [`DetachedWork`], which the transport stops at the close
//! of its shutdown window.

use std::any::TypeId;
use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest,
};
use async_graphql::{Request, ServerResult};
use nest_rs_core::{Container, ReachableProviders, TaskContext};
use nest_rs_http::DetachedWork;

/// One DataLoader registration, module-gated by its owner's reachability:
/// `container.get::<Owner>()` would panic at request time otherwise.
#[doc(hidden)]
pub struct GraphqlLoaderRegistration {
    pub owner_type_id: fn() -> TypeId,
    pub seed: fn(&Container, &DetachedWork, Request) -> Request,
}

inventory::collect!(GraphqlLoaderRegistration);

/// A DataLoader batch's work, boxed for spawning on its own task.
pub type GraphqlBatchFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

/// Spawns a batch future, having re-established the request's ambient state
/// around it (see [`GraphqlBatchContext`]).
pub type GraphqlBatchSpawner = Box<dyn Fn(GraphqlBatchFuture) + Send + Sync>;

/// Re-establishes per-request ambient state (executor, ability) inside a
/// DataLoader batch, which async-graphql runs on a task with empty task-locals.
/// `spawner` is called per request inside the operation's ambient scope.
///
/// Bind with `providers = [MyBridge as dyn GraphqlBatchContext]`, implemented by
/// `nest_rs_seaorm::graphql::LoaderScope`. Without one, batches spawn bare and a
/// loader reading through `Repo` fails; the schema build warns.
pub trait GraphqlBatchContext: Send + Sync + 'static {
    /// Build a spawner that carries the current request's ambient executor +
    /// ability into each batch it runs.
    fn spawner(&self) -> GraphqlBatchSpawner;
}

#[doc(hidden)]
pub fn batch_spawner(container: &Container, batches: &DetachedWork) -> GraphqlBatchSpawner {
    let inner = match container.get_dyn::<dyn GraphqlBatchContext>() {
        Some(ctx) => ctx.spawner(),
        None => Box::new(|fut| {
            tokio::spawn(fut);
        }),
    };
    instrumented(inner, batches.clone())
}

/// Carry the request's [`TaskContext`] across the batch's task boundary, and
/// run the batch as `batches` work, whatever the registered context does with
/// the future otherwise — the one seam every batch goes through.
fn instrumented(inner: GraphqlBatchSpawner, batches: DetachedWork) -> GraphqlBatchSpawner {
    Box::new(move |fut| {
        let carried = batches.clone();
        inner(Box::pin(async move {
            let _ = carried.run(TaskContext::current().carry(fut)).await;
        }));
    })
}

/// The per-request seeding step of one reachable dataloader.
type LoaderSeed = fn(&Container, &DetachedWork, Request) -> Request;

/// Seeds every discovered DataLoader into each GraphQL request.
pub(crate) struct LoaderExtensionFactory {
    container: Container,
    /// The reachable loaders, resolved **once** at schema build.
    seeds: Arc<[LoaderSeed]>,
    /// What the batches are carried by — the `/graphql` mount's.
    batches: DetachedWork,
}

impl LoaderExtensionFactory {
    pub(crate) fn new(container: Container, batches: DetachedWork) -> Self {
        warn_unreachable_loaders(&container);
        let seeds = reachable_seeds(&container);
        warn_missing_batch_context(&container, seeds.len());
        Self {
            container,
            seeds,
            batches,
        }
    }
}

/// Freeze the reachable loader seeds. Without `ReachableProviders` (a
/// hand-rolled container) nothing is seeded — fail-closed, reported once.
fn reachable_seeds(container: &Container) -> Arc<[LoaderSeed]> {
    let Some(reachable) = container.get::<ReachableProviders>() else {
        tracing::warn!(
            target: crate::TARGET,
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

/// Warn once at schema build about `#[dataloader]`s linked into the binary
/// whose owner's module is unreachable. Only a count: a registration carries
/// the owner's `TypeId`, not a name.
fn warn_unreachable_loaders(container: &Container) {
    // Without a gate, `reachable_seeds` already warned.
    let Some(reachable) = container.get::<ReachableProviders>() else {
        return;
    };
    let skipped = inventory::iter::<GraphqlLoaderRegistration>()
        .filter(|reg| !reachable.0.contains(&(reg.owner_type_id)()))
        .count();
    if skipped > 0 {
        tracing::warn!(
            target: crate::TARGET,
            count = skipped,
            hint = "import the modules that provide these loaders; relation fields backed by them error at query time",
            "dataloaders linked but unreachable",
        );
    }
}

/// Warn once at schema build when loaders are seeded but nothing re-installs
/// the request's ambient state around their batches: a `Repo` read then fails
/// before any SQL is issued.
fn warn_missing_batch_context(container: &Container, seeded: usize) {
    const HINT: &str = "list `LoaderScope as dyn GraphqlBatchContext` in a reachable module \
         (`nestrs g graphql` writes it into authz/graphql/) — without it every batch runs on a \
         task with no ambient executor, and each relation field answers with an error before \
         any SQL is issued";
    if seeded == 0 || container.get_dyn::<dyn GraphqlBatchContext>().is_some() {
        return;
    }
    tracing::warn!(
        target: crate::TARGET,
        loaders = seeded,
        hint = HINT,
        "dataloaders seeded with no batch context",
    );
}

impl ExtensionFactory for LoaderExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(LoaderExtension {
            container: self.container.clone(),
            seeds: Arc::clone(&self.seeds),
            batches: self.batches.clone(),
        })
    }
}

struct LoaderExtension {
    container: Container,
    seeds: Arc<[LoaderSeed]>,
    batches: DetachedWork,
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
            request = seed(&self.container, &self.batches, request);
        }
        next.run(ctx, request).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn a_container_with_no_reachability_says_it_seeded_no_loaders() {
        let logs = nest_rs_testing::LogCapture::install();
        // `App::builder`/`App::new` always seed `ReachableProviders`.
        let seeds = reachable_seeds(&Container::builder().build());
        assert!(seeds.is_empty(), "nothing is seeded without reachability");

        let event = logs.expect_one(
            "nest_rs::graphql",
            "loaders skipped: no ReachableProviders seeded",
        );
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("hint")
                .is_some_and(|h| h.contains("App::builder")),
            "the remedy is the point of the line, got {:?}",
            event.fields,
        );
    }

    /// The owner of a loader no module provides. Link-time, so it is in every
    /// test of this binary, inert: `reachable_seeds` filters it out.
    struct AbsentOwner;

    inventory::submit! {
        GraphqlLoaderRegistration {
            owner_type_id: || TypeId::of::<AbsentOwner>(),
            seed: |_, _, request| request,
        }
    }

    /// `ReachableProviders` seeded with exactly `types`.
    fn reached(types: &[TypeId]) -> Container {
        Container::builder()
            .provide(ReachableProviders(types.iter().copied().collect()))
            .build()
    }

    #[test]
    fn a_loader_whose_owner_module_is_not_imported_is_counted_at_boot() {
        let logs = nest_rs_testing::LogCapture::install();
        warn_unreachable_loaders(&reached(&[]));

        let event = logs.expect_one("nest_rs::graphql", "dataloaders linked but unreachable");
        assert_eq!(event.level, "warn");
        // At least one: the link-time count is a property of the whole test binary.
        let counted: usize = event
            .field("count")
            .and_then(|c| c.parse().ok())
            .unwrap_or_default();
        assert!(
            counted >= 1,
            "the skipped loader is counted: {:?}",
            event.fields,
        );
        assert!(
            event.field("hint").is_some_and(|h| h.contains("import")),
            "the remedy is the point of the line, got {:?}",
            event.fields,
        );
    }

    #[test]
    fn a_loader_whose_owner_is_reachable_is_not_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = reached(&[TypeId::of::<AbsentOwner>()]);
        warn_unreachable_loaders(&container);
        logs.expect_none("nest_rs::graphql", "dataloaders linked but unreachable");
        assert!(
            !reachable_seeds(&container).is_empty(),
            "and it is seeded rather than skipped",
        );
    }

    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test's receiver may already be gone"
    )]
    async fn batch_spawner_without_a_context_runs_the_future_on_tokio_spawn() {
        let container = Container::builder().build();
        let spawner = batch_spawner(&container, &DetachedWork::new());
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
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test's receiver may already be gone"
    )]
    async fn batch_spawner_routes_through_a_registered_batch_context() {
        let count = Arc::new(AtomicUsize::new(0));
        let ctx: Arc<dyn GraphqlBatchContext> = Arc::new(CountingContext {
            count: count.clone(),
        });
        let container = Container::builder().provide_dyn(ctx).build();

        let spawner = batch_spawner(&container, &DetachedWork::new());
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

#[cfg(test)]
mod batch_context_warning {
    use nest_rs_testing::LogCapture;

    use super::*;

    struct Bridge;

    impl GraphqlBatchContext for Bridge {
        fn spawner(&self) -> GraphqlBatchSpawner {
            Box::new(|fut| {
                tokio::spawn(fut);
            })
        }
    }

    const EVENT: &str = "dataloaders seeded with no batch context";

    #[test]
    fn seeded_loaders_without_a_batch_context_warn_at_schema_build() {
        let logs = LogCapture::install();
        warn_missing_batch_context(&Container::builder().build(), 3);

        let event = logs.expect_one("nest_rs::graphql", EVENT);
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("loaders").as_deref(), Some("3"));
        assert!(
            event
                .field("hint")
                .is_some_and(|h| h.contains("LoaderScope as dyn GraphqlBatchContext")),
            "the hint names the binding: {event:#?}",
        );
    }

    #[test]
    fn a_bound_batch_context_is_silent() {
        let container = Container::builder()
            .provide_dyn::<dyn GraphqlBatchContext>(Arc::new(Bridge))
            .build();
        let logs = LogCapture::install();
        warn_missing_batch_context(&container, 3);
        logs.expect_none("nest_rs::graphql", EVENT);
    }

    #[test]
    fn no_loaders_means_no_warning() {
        let logs = LogCapture::install();
        warn_missing_batch_context(&Container::builder().build(), 0);
        logs.expect_none("nest_rs::graphql", EVENT);
    }
}

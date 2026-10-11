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
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest,
};
use async_graphql::{Request, ServerResult};
use nest_rs_core::{Container, Continuation, ReachableProviders, TaskContext};
use nest_rs_http::DetachedWork;

/// One DataLoader registration, module-gated by its owner's reachability:
/// `container.get::<Owner>()` would panic at request time otherwise.
pub struct GraphqlLoaderRegistration {
    /// The `#[dataloader]` owner's type, read against the reachable providers.
    pub owner_type_id: fn() -> TypeId,
    /// Attach the owner's loaders to the request's data.
    pub seed: fn(&Container, &DetachedWork, Request) -> Request,
}

inventory::collect!(GraphqlLoaderRegistration);

/// A DataLoader batch's work, as async-graphql hands it to the spawner.
type BatchFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// The spawner a request's DataLoader batches run through, filed under
/// `batches`.
///
/// async-graphql calls it on the resolver's task, inside `load_many`, so the
/// capture taken there sees the request: the batch runs under what the request
/// carries into a [`Batch`](Continuation::Batch) — its trace, the caller's
/// ability, the pool rather than its transaction — whichever field triggered
/// it.
pub fn batch_spawner(batches: &DetachedWork) -> impl Fn(BatchFuture) + Send + Sync + 'static {
    let batches = batches.clone();
    move |batch| {
        let carried = TaskContext::capture(Continuation::Batch);
        let batches = batches.clone();
        tokio::spawn(async move {
            let _ = batches.run(carried.carry(batch)).await;
        });
    }
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

    /// A value the triggering unit carries, read back by the batch.
    #[derive(Debug, PartialEq)]
    struct Caller(&'static str);

    impl nest_rs_core::AmbientValue for Caller {}

    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test's receiver may already be gone"
    )]
    async fn a_batch_runs_under_what_its_unit_carried_when_it_was_spawned() {
        let spawner = batch_spawner(&DetachedWork::new());
        let correlation = nest_rs_core::Correlation::minted(None);
        let trace = correlation.trace_id();
        let (tx, rx) = tokio::sync::oneshot::channel();
        nest_rs_core::with_request_scope(None, correlation, async {
            nest_rs_core::with_ambient(Arc::new(Caller("alice")), async {
                spawner(Box::pin(async move {
                    let _ = tx.send((
                        nest_rs_core::current_trace_id(),
                        nest_rs_core::ambient::<Caller>(),
                    ));
                }));
            })
            .await;
        })
        .await;

        let (seen_trace, seen_caller) = rx.await.expect("the batch runs");
        assert_eq!(seen_trace, Some(trace), "the batch keeps its unit's trace");
        assert_eq!(seen_caller.as_deref(), Some(&Caller("alice")));
    }
}

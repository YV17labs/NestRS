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
/// registered, batches spawn bare on `tokio::spawn` — correct only for a
/// loader that reaches no request-scoped state. A loader going through `Repo`
/// (which every auto-emitted relation loader does) finds no ambient executor on
/// that bare task and fails the whole relation before a single statement
/// reaches the database, so the schema build warns when loaders exist and no
/// context is bound.
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
        warn_missing_batch_context(&container, seeds.len());
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

/// Warn once at schema build when loaders are seeded but nothing re-installs
/// the request's ambient state around their batches.
///
/// This is the boot signal a whole class of failure had none of. Every
/// auto-emitted relation loader reads through `Repo`, and `Repo` runs against
/// the *ambient* executor — which a bare `tokio::spawn` batch does not have.
/// The result was a relation that returned `database error` with no `DbErr`
/// behind it and, tellingly, **no SQL reaching the database at all**: the query
/// failed before execution, on a wiring gap nothing announced. The flagship
/// "relations resolve themselves" feature looked broken with nothing to grep.
fn warn_missing_batch_context(container: &Container, seeded: usize) {
    const HINT: &str = "list `LoaderScope as dyn GraphqlBatchContext` in a reachable module \
         (`nestrs g graphql` writes it into authz/graphql/) — without it every batch runs on a \
         task with no ambient executor, and each relation field answers with an error before \
         any SQL is issued";
    if seeded == 0 || container.get_dyn::<dyn GraphqlBatchContext>().is_some() {
        return;
    }
    tracing::warn!(
        target: "nest_rs::graphql",
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
    /// A container with no `ReachableProviders` seeds **no** loaders — the
    /// fail-closed answer, since resolving a loader whose owner module is
    /// absent would panic on `container.get::<Owner>()`.
    ///
    /// What makes the event load-bearing is that nothing else changes: the
    /// schema builds, every query works, and only a relation field backed by a
    /// loader errors — at query time, in production, to a client. This line is
    /// emitted once at build and carries the two ways to fix it.
    #[test]
    fn a_container_with_no_reachability_says_it_seeded_no_loaders() {
        let logs = nest_rs_testing::LogCapture::install();
        // Hand-rolled: `App::builder`/`App::new` always seed `ReachableProviders`,
        // so this is the one shape that reaches the branch.
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

    /// The owner of a loader no module in the app provides. `inventory` is
    /// link-time, so this registration is in the test binary whether a given
    /// test wants it or not — which *is* the situation the event reports, and
    /// it is inert: `reachable_seeds` filters it out of every seed list.
    struct AbsentOwner;

    inventory::submit! {
        GraphqlLoaderRegistration {
            owner_type_id: || TypeId::of::<AbsentOwner>(),
            seed: |_, request| request,
        }
    }

    /// `ReachableProviders` seeded with exactly `types` — the shape a real boot
    /// leaves behind, as opposed to the no-gate shape above.
    fn reached(types: &[TypeId]) -> Container {
        Container::builder()
            .provide(ReachableProviders(types.iter().copied().collect()))
            .build()
    }

    #[test]
    fn a_loader_whose_owner_module_is_not_imported_is_counted_at_boot() {
        // The failure this exists for is silent by construction: the schema
        // builds, every query answers, and only a relation field backed by the
        // skipped loader errors — at query time, to a client, in an app whose
        // boot said nothing. `#[dataloader]` on a service in a feature crate
        // that this binary links but does not import is all it takes.
        let logs = nest_rs_testing::LogCapture::install();
        warn_unreachable_loaders(&reached(&[]));

        let event = logs.expect_one("nest_rs::graphql", "dataloaders linked but unreachable");
        assert_eq!(event.level, "warn");
        // At least one, not exactly one: `inventory` is link-time, so the count
        // is a property of the whole test binary. Pinning it to `1` would break
        // the day this crate gains a second `#[cfg(test)]` registration or its
        // first unit test of a real `#[dataloader]` — silently, on a number
        // that was never this test's subject.
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
        // The other direction, and the one that keeps the count honest: every
        // app that works links loaders, so a check reading the registry alone
        // would warn on all of them.
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

    /// The wiring gap behind "relations resolve themselves, except they return
    /// `database error`". Every auto-emitted relation loader reads through
    /// `Repo`, `Repo` runs against the *ambient* executor, and a batch spawned
    /// bare has none — so the relation failed before a single statement reached
    /// the database, with nothing at `RUST_LOG=trace` to point at the cause.
    /// Schema build now names it, and names the binding that fixes it.
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

    /// Bound, and the warning goes away — otherwise it would be noise every
    /// correctly-wired app has to learn to ignore.
    #[test]
    fn a_bound_batch_context_is_silent() {
        let container = Container::builder()
            .provide_dyn::<dyn GraphqlBatchContext>(Arc::new(Bridge))
            .build();
        let logs = LogCapture::install();
        warn_missing_batch_context(&container, 3);
        logs.expect_none("nest_rs::graphql", EVENT);
    }

    /// An app with no loaders at all needs no context — a schema of plain
    /// resolvers must boot silent.
    #[test]
    fn no_loaders_means_no_warning() {
        let logs = LogCapture::install();
        warn_missing_batch_context(&Container::builder().build(), 0);
        logs.expect_none("nest_rs::graphql", EVENT);
    }
}

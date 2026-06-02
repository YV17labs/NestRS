//! Request-scoped DataLoaders, discovered at link time.
//!
//! `#[dataloader]` on a data-layer impl block generates one batching loader per
//! method and submits a [`LoaderRegistration`] here. Rather than living in the
//! DI container as a single shared instance, each loader is rebuilt *per
//! request* and seeded into the GraphQL context by [`LoaderExtension`]: a
//! `#[field]` then reads it back as `&DataLoader<…>`. This mirrors NestJS's
//! request-scoped loaders, lets a loader observe per-request state, and — the
//! point — makes module import order irrelevant: the loader is built from the
//! fully assembled container when a request arrives, never at registration time.

use std::any::TypeId;
use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest,
};
use async_graphql::{Request, ServerResult};
use nestrs_core::{Container, ReachableProviders};

/// One DataLoader, submitted by `#[dataloader]`. `seed` builds a fresh loader
/// from the (complete) container and attaches it to the request as context
/// data. `owner_type_id` is the `TypeId` of the service the loader's
/// `from_container` reads (the `Self` of the `#[dataloader]` impl block) —
/// when the owner is not in [`ReachableProviders`], the loader's
/// `container.get::<Self>()` would panic at request time, so this seed is
/// module-gated by the owner's reachability. `pub` only so the generated code
/// can name it.
#[doc(hidden)]
pub struct LoaderRegistration {
    pub owner_type_id: fn() -> TypeId,
    pub seed: fn(&Container, Request) -> Request,
}

inventory::collect!(LoaderRegistration);

/// A boxed batch future — the currency a [`BatchContext`] spawner accepts, matching
/// async-graphql's `DataLoader::new` spawner signature (`Fn(BoxFuture<'static, ()>)`).
pub type BatchFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

/// The spawner async-graphql's `DataLoader` calls to run a queued batch.
pub type BatchSpawner = Box<dyn Fn(BatchFuture) + Send + Sync>;

/// The seam that re-establishes per-request ambient state inside a DataLoader
/// batch. async-graphql runs every batch on a *spawned* task (so concurrent
/// `load_one`s collapse into one query), and a spawned task starts with empty
/// task-local storage — so the ambient executor and authorization ability a
/// request installs are gone by the time a batch loads, and a loader's `Repo`
/// reads would run unscoped. An implementor [`spawner`](BatchContext::spawner)
/// is called *per request* while building each loader — inside the operation's
/// ambient scope, so it can snapshot that state — and returns a spawner that
/// re-installs it around every batch future.
///
/// Bind an implementor with `providers = [MyBridge as dyn BatchContext]`; the
/// loader seed resolves it via the container ([`Container::get_dyn`]). With none
/// registered, batches spawn bare on `tokio::spawn` (loaders run unscoped — the
/// prior behaviour, correct for an app without row-level security).
pub trait BatchContext: Send + Sync + 'static {
    /// Build the spawner for the loaders of one request. Called inside the
    /// operation's ambient scope, so the implementor reads the live task-locals
    /// here and captures them into the returned closure.
    fn spawner(&self) -> BatchSpawner;
}

/// The batch spawner for this request's loaders: the bound [`BatchContext`]'s,
/// or a bare `tokio::spawn` when none is registered. Called from the
/// `#[dataloader]`-generated seed; `pub` only so that generated code can name it.
#[doc(hidden)]
pub fn batch_spawner(container: &Container) -> BatchSpawner {
    match container.get_dyn::<dyn BatchContext>() {
        Some(ctx) => ctx.spawner(),
        None => Box::new(|fut| {
            tokio::spawn(fut);
        }),
    }
}

/// Seeds every discovered DataLoader into each GraphQL request. Built by
/// [`build_schema`](crate::resolver::build_schema) with a clone of the app
/// container; one [`LoaderExtension`] is created per request.
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
        // Module-gate the inventory: seed only the loaders whose owner service
        // is in this app's reachable provider set. A loader from an unimported
        // module would panic on `container.get::<Owner>()` — skipping its seed
        // mirrors the GraphQL resolver filter and keeps cross-app discovery
        // silent.
        let reachable = self.container.get::<ReachableProviders>();
        for reg in inventory::iter::<LoaderRegistration>() {
            if let Some(ref r) = reachable {
                if !r.0.contains(&(reg.owner_type_id)()) {
                    continue;
                }
            }
            request = (reg.seed)(&self.container, request);
        }
        next.run(ctx, request).await
    }
}

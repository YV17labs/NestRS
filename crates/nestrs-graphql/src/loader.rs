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

use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest,
};
use async_graphql::{Request, ServerResult};
use nestrs_core::Container;

/// One DataLoader, submitted by `#[dataloader]`. `seed` builds a fresh loader
/// from the (complete) container and attaches it to the request as context data.
/// `pub` only so the generated code can name it.
#[doc(hidden)]
pub struct LoaderRegistration {
    pub seed: fn(&Container, Request) -> Request,
}

inventory::collect!(LoaderRegistration);

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
        for reg in inventory::iter::<LoaderRegistration>() {
            request = (reg.seed)(&self.container, request);
        }
        next.run(ctx, request).await
    }
}

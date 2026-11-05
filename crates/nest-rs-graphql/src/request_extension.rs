//! What the schema does around each request, in the one extension it keeps,
//! since an extension is entered on every field (`redact.rs`): it seeds the
//! request's loaders, and once validation passed answers the refusal the
//! operation's read left ([`OperationRefusal`]). No other hook sits between
//! validation and execution.

use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest, NextValidation,
};
use async_graphql::{Request, ServerError, ServerResult, ValidationResult};

use crate::loader::LoaderSeeds;
use crate::operation_units::OperationRefusal;

/// Builds each request's [`RequestExtension`].
pub(crate) struct RequestExtensionFactory(Arc<LoaderSeeds>);

impl RequestExtensionFactory {
    pub(crate) fn new(seeds: LoaderSeeds) -> Self {
        Self(Arc::new(seeds))
    }
}

impl ExtensionFactory for RequestExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(RequestExtension(Arc::clone(&self.0)))
    }
}

struct RequestExtension(Arc<LoaderSeeds>);

#[async_trait]
impl Extension for RequestExtension {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        next.run(ctx, self.0.seed(request)).await
    }

    async fn validation(
        &self,
        ctx: &ExtensionContext<'_>,
        next: NextValidation<'_>,
    ) -> Result<ValidationResult, Vec<ServerError>> {
        let validated = next.run(ctx).await?;
        match ctx
            .data_opt::<OperationRefusal>()
            .and_then(OperationRefusal::said)
        {
            Some(refusal) => Err(vec![refusal]),
            None => Ok(validated),
        }
    }
}

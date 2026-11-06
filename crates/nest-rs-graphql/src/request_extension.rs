//! What the schema does around each request, in the one extension it keeps,
//! since an extension is entered on every field (`redact.rs`): before anything
//! walks the document it refuses what the document would cost
//! ([`document_cost`]), it seeds the request's loaders and what it files once
//! ([`FiledOnce`]), and once validation passed answers the refusal the operation's
//! read left ([`OperationRefusal`]). No other hook sits between validation and
//! execution.

use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest, NextValidation,
};
use async_graphql::{Request, ServerError, ServerResult, ValidationResult};

use crate::document_cost::{self, DocumentLimits};
use crate::filed_once::FiledOnce;
use crate::loader::LoaderSeeds;
use crate::operation_units::OperationRefusal;

/// Hands each request the schema's one [`RequestExtension`], which holds nothing of
/// a request's.
pub(crate) struct RequestExtensionFactory(Arc<RequestExtension>);

impl RequestExtensionFactory {
    /// Each request seeded from `seeds`, what spreading a fragment again adds to its
    /// document held to `repeated_selections`.
    pub(crate) fn new(seeds: LoaderSeeds, repeated_selections: usize) -> Self {
        Self(Arc::new(RequestExtension {
            seeds,
            repeated_selections,
        }))
    }
}

impl ExtensionFactory for RequestExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        self.0.clone()
    }
}

struct RequestExtension {
    seeds: LoaderSeeds,
    repeated_selections: usize,
}

impl RequestExtension {
    /// What a document may add to its text. A request's arguments may copy twice what
    /// it may carry, so every variable may be referred to twice: past that, a request's
    /// few references to its own value would multiply it.
    fn limits(&self) -> DocumentLimits {
        DocumentLimits {
            repeated_selections: self.repeated_selections,
            copied_bytes: crate::subscription::current_request_limit().saturating_mul(2),
        }
    }
}

#[async_trait]
impl Extension for RequestExtension {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        mut request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        let variables = std::mem::take(&mut request.variables);
        let operation = request.operation_name.take();
        // The operation's read cached this parse; one that fails is answered by
        // async-graphql's own, ahead.
        let refusal = request.parsed_query().ok().and_then(|document| {
            document_cost::refusal(document, operation.as_deref(), &variables, self.limits())
        });
        request.variables = variables;
        request.operation_name = operation;
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        next.run(ctx, self.seeds.seed(request).data(FiledOnce::default()))
            .await
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

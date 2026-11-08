//! What the schema does around each request, in the one extension it keeps,
//! since an extension is entered on every field (`redact.rs`): before anything
//! walks the document it refuses one that does not parse ([`unparsed`]) and what
//! one that does would cost ([`document_cost`]), gives each object that could answer
//! no key one of its own ([`OwnKey`]), seeds the request's loaders and what it files
//! once ([`FiledOnce`]), and once validation passed answers the refusal the
//! operation's read or that key's left ([`OperationRefusal`]). No other hook sits
//! between validation and execution.

use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest, NextValidation,
};
use async_graphql::{Request, ServerError, ServerResult, ValidationResult};

use crate::config::GraphqlConfig;
use crate::context::held_by;
use crate::document_cost::{self, DocumentLimits};
use crate::filed_once::FiledOnce;
use crate::loader::LoaderSeeds;
use crate::operation_units::{OperationRefusal, selected_operation};
use crate::own_key::OwnKey;
use crate::redact::unparsed;

/// Hands each request the schema's one [`RequestExtension`], which holds nothing of
/// a request's.
pub(crate) struct RequestExtensionFactory(Arc<RequestExtension>);

impl RequestExtensionFactory {
    /// Each request seeded from `seeds`, what its document adds to its text held to
    /// what `config` bounds.
    pub(crate) fn new(seeds: LoaderSeeds, config: &GraphqlConfig) -> Self {
        Self(Arc::new(RequestExtension {
            seeds,
            repeated_selections: config.repeated_selections(),
            list_argument_bytes: config.max_list_argument_bytes.unwrap_or(usize::MAX),
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
    list_argument_bytes: usize,
}

impl RequestExtension {
    /// What a document may add to its text. A request's arguments may copy twice what
    /// it may carry, so every variable may be referred to twice: past that, a request's
    /// few references to its own value would multiply it.
    fn limits(&self) -> DocumentLimits {
        DocumentLimits {
            repeated_selections: self.repeated_selections,
            copied_bytes: crate::subscription::current_request_limit().saturating_mul(2),
            list_argument_bytes: self.list_argument_bytes,
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
        let own_key = held_by::<OwnKey>(&request.data).cloned();
        let variables = std::mem::take(&mut request.variables);
        let operation = request.operation_name.take();
        let registry = &ctx.schema_env.registry;
        // The operation's read cached a parse that passed; one that failed runs again
        // here, and the refusal says only what this one left. Its objects are given
        // their keys once its cost is counted, which bounds what that read expands,
        // and copies no key joins are refused once validation passed.
        let mut unjoined = None;
        let read = match request.parsed_query() {
            Ok(document) => match document_cost::refusal(
                document,
                operation.as_deref(),
                &variables,
                self.limits(),
                registry,
            ) {
                Some(refusal) => Err(refusal),
                None => Ok(own_key
                    .zip(selected_operation(document, operation.as_deref()))
                    .and_then(|(own_key, (_, selected))| {
                        own_key
                            .give(document, &selected.node, registry, &variables)
                            .unwrap_or_else(|refusal| {
                                unjoined = Some(refusal);
                                None
                            })
                    })),
            },
            Err(refused) => Err(unparsed(refused)),
        };
        request.variables = variables;
        request.operation_name = operation;
        if let Some(given) = read? {
            request.set_parsed_query(given);
        }
        if let Some(refusal) = unjoined
            && held_by::<OperationRefusal>(&request.data).is_none()
        {
            request.data.insert(OperationRefusal::new(refusal));
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
            .map(OperationRefusal::said)
        {
            Some(refusal) => Err(vec![refusal]),
            None => Ok(validated),
        }
    }
}

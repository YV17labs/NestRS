//! What the schema does around each request, in the one extension it keeps,
//! since an extension is entered on every field (`redact.rs`): before anything
//! walks the document it refuses one that does not parse ([`unparsed`]) and what
//! one that does would cost ([`document_cost`]), collects it as GraphQL §6.3.2 reads
//! it and gives each object that could answer no key one of its own ([`OwnKey`]),
//! seeds the request's loaders and what it files once ([`FiledFacts`]), and once
//! validation passed, each of its errors said once, answers the refusal the
//! operation's read or that collection left ([`OperationRefusal`]). No other hook
//! sits between validation and execution.

use std::sync::Arc;

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest, NextValidation,
};
use async_graphql::{Request, ServerError, ServerResult, ValidationResult};

use crate::config::GraphqlConfig;
use crate::context::held_by;
use crate::document_cost::{self, DocumentLimits};
use crate::field_collection;
use crate::filed_facts::FiledFacts;
use crate::loader::LoaderSeeds;
use crate::operation_units::{OperationRefusal, selected_operation};
use crate::own_key::OwnKey;
use crate::redact::unparsed;
use crate::root_fields::RootFields;

/// Hands each request the schema's one [`RequestExtension`], which holds nothing of
/// a request's.
pub(crate) struct RequestExtensionFactory(Arc<RequestExtension>);

impl RequestExtensionFactory {
    /// Each request seeded from `seeds`, what its document adds to its text held to
    /// what `config` bounds.
    pub(crate) fn new(seeds: Arc<LoaderSeeds>, config: &GraphqlConfig) -> Self {
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
    seeds: Arc<LoaderSeeds>,
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
        // here, and the refusal says only what this one left. It is collected, and its
        // objects given their keys, once its cost is counted, which bounds what that
        // read expands, and copies no one field answers are refused once validation
        // passed.
        let mut uncollected = None;
        let limits = self.limits();
        let read = match request.parsed_query() {
            Ok(document) => match document_cost::refusal(
                document,
                operation.as_deref(),
                &variables,
                limits,
                registry,
            ) {
                Some(refusal) => Err(refusal),
                None => Ok(selected_operation(document, operation.as_deref()).and_then(
                    |(_, selected)| {
                        // A schema answering with no key of its own still runs a root
                        // field once.
                        let read = match &own_key {
                            Some(own_key) => {
                                own_key.give(document, &selected.node, registry, &variables, limits)
                            }
                            None => field_collection::collected(
                                document,
                                &selected.node,
                                registry,
                                &variables,
                                limits,
                            ),
                        };
                        read.unwrap_or_else(|refusal| {
                            uncollected = Some(refusal);
                            None
                        })
                    },
                )),
            },
            Err(refused) => Err(unparsed(refused)),
        };
        request.variables = variables;
        request.operation_name = operation;
        if let Some(given) = read? {
            request.set_parsed_query(given);
        }
        if let Some(refusal) = uncollected
            && held_by::<OperationRefusal>(&request.data).is_none()
        {
            request.data.insert(OperationRefusal::new(refusal));
        }
        // The operation's unit hands one in, to read back the root fields its
        // answer's errors were met under; a schema executed bare gets its own.
        if held_by::<Arc<RootFields>>(&request.data).is_none() {
            request.data.insert(Arc::new(RootFields::default()));
        }
        next.run(
            ctx,
            self.seeds
                .seed(request)
                .data(Arc::new(FiledFacts::default())),
        )
        .await
    }

    async fn validation(
        &self,
        ctx: &ExtensionContext<'_>,
        next: NextValidation<'_>,
    ) -> Result<ValidationResult, Vec<ServerError>> {
        let validated = next.run(ctx).await.map_err(said_once)?;
        match ctx
            .data_opt::<OperationRefusal>()
            .map(OperationRefusal::said)
        {
            Some(refusal) => Err(vec![refusal]),
            None => Ok(validated),
        }
    }
}

/// `errors` each said once: a fragment the operation's spreads were inlined from
/// ([`crate::field_collection`]) is judged at each of them, where the document
/// sent was judged once.
fn said_once(mut errors: Vec<ServerError>) -> Vec<ServerError> {
    let mut said = std::collections::HashSet::with_capacity(errors.len());
    errors.retain(|error| said.insert((error.message.clone(), error.locations.clone())));
    errors
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_graphql::{EmptySubscription, Object, Schema};
    use nest_rs_core::module;
    use nest_rs_http::DetachedWork;
    use serde_json::json;

    use super::*;
    use crate::loader::LoaderSeeds;

    #[module]
    struct Bare;

    /// How many writes [`Mutation`] ran, in this test's process.
    static WRITES: AtomicUsize = AtomicUsize::new(0);

    struct Query;

    #[Object]
    impl Query {
        async fn up(&self) -> bool {
            true
        }
    }

    struct Mutation;

    #[Object]
    impl Mutation {
        async fn write(&self) -> usize {
            WRITES.fetch_add(1, Ordering::SeqCst) + 1
        }
    }

    /// A schema executed with no key of its own to give, outside the executor
    /// every mount wraps it in, still collects its document: a root field selected
    /// twice runs once.
    #[tokio::test]
    async fn a_schema_holding_no_key_still_runs_a_root_field_once() {
        let app = nest_rs_testing::TestApp::builder()
            .module::<Bare>()
            .build_headless()
            .await
            .expect("an empty app boots");
        let loaders =
            LoaderSeeds::new(app.container().clone(), DetachedWork::new()).expect("sealed");
        let schema = Schema::build(Query, Mutation, EmptySubscription)
            .extension(RequestExtensionFactory::new(
                Arc::new(loaders),
                &GraphqlConfig::default(),
            ))
            .finish();
        let response = schema.execute("mutation { a: write a: write }").await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(WRITES.load(Ordering::SeqCst), 1);
        assert_eq!(
            response.data.into_json().expect("plain JSON"),
            json!({ "a": 1 })
        );
    }
}

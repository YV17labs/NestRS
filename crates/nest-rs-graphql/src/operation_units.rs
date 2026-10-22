//! Every operation a GraphQL request carries runs as a unit of work of its
//! own, [`OPERATION`](crate::unit::OPERATION), through the binding the mount
//! resolves: a query on the pool, a mutation in one transaction whose root
//! fields each run in an atomic block settled on the field's own answer, so a
//! field that answers `null` for an error rolls back alone while the
//! operation's other fields commit, and an operation whose answer reports no
//! data rolls back whole — a response never reports work that was rolled back,
//! nor stays silent on work that stands.

use std::convert::identity;
use std::future::Future;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use async_graphql::futures_util::StreamExt;
use async_graphql::futures_util::stream::{self, BoxStream, FuturesOrdered};
use async_graphql::parser::types::{
    DocumentOperations, ExecutableDocument, OperationDefinition, OperationType,
};
use async_graphql::{
    BatchRequest, BatchResponse, Context, Data, Executor, Name, Positioned, Request, Response,
    ServerError, ServerResult, Value,
};
use nest_rs_core::operation_log::{CANCELLED, ERROR, OK, PANIC};
use nest_rs_core::problem::{Problem, code};
use nest_rs_database::{
    AtomicError, ExecutorScope, TransactionPolicy, Unhonoured, UnitOfWork, UnitSpec, run_unit,
};
use nest_rs_http::RequestDeadline;
use tracing::Instrument;

use crate::loader::BatchSpan;
use crate::null_propagation::{MutationAnswer, RootAnswer};

/// The schema `E`, each query and mutation it executes run as a unit of work
/// of its own; a subscription's unit is its connection.
#[derive(Clone)]
pub(crate) struct OperationUnits<E> {
    executor: E,
    /// The unit-of-work binding, resolved once at mount; with none, every
    /// operation runs bare.
    binding: Option<Arc<dyn UnitOfWork>>,
}

impl<E: Executor> OperationUnits<E> {
    pub(crate) fn new(executor: E, binding: Option<Arc<dyn UnitOfWork>>) -> Self {
        Self { executor, binding }
    }

    /// Execute `batch` within the request's `deadline`: one operation at a
    /// time, in document order, when any of them is a mutation, so its units
    /// never hold several pooled connections at once; all at once otherwise.
    pub(crate) async fn execute_batch_within(
        &self,
        batch: BatchRequest,
        deadline: Option<&RequestDeadline>,
    ) -> BatchResponse {
        match batch {
            BatchRequest::Single(mut request) => {
                let selected = Selected::of(&mut request);
                let cut = self.cut(selected.commits(), deadline);
                BatchResponse::Single(self.execute_selected(selected, request, cut).await)
            }
            BatchRequest::Batch(requests) => {
                let selections: Vec<(Selected, Request)> = requests
                    .into_iter()
                    .map(|mut request| (Selected::of(&mut request), request))
                    .collect();
                let in_order = selections.iter().any(|(selected, _)| selected.commits());
                let cut = self.cut(in_order, deadline);
                let operations = selections
                    .into_iter()
                    .map(|(selected, request)| self.execute_selected(selected, request, cut));
                BatchResponse::Batch(match in_order {
                    true => stream::iter(operations).then(identity).collect().await,
                    false => operations.collect::<FuturesOrdered<_>>().collect().await,
                })
            }
        }
    }

    /// Where a batch's operations are cut inside their units: at the request's
    /// deadline, taken over from the edge, when the batch can commit, since a
    /// commit is never cut; otherwise the edge cuts the batch.
    fn cut<'d>(
        &self,
        commits: bool,
        deadline: Option<&'d RequestDeadline>,
    ) -> Option<&'d RequestDeadline> {
        match (&self.binding, deadline) {
            (Some(binding), Some(deadline)) if commits => {
                deadline.hand_to_unit(binding.settle_net());
                Some(deadline)
            }
            _ => None,
        }
    }

    async fn execute_selected(
        &self,
        selected: Selected,
        request: Request,
        cut: Option<&RequestDeadline>,
    ) -> Response {
        let request = request.data(BatchSpan(tracing::Span::current()));
        self.run(selected, cut, self.executor.execute(request))
            .await
    }

    /// Run `body`, the operation `selected` names, as its unit: cut at `cut`,
    /// settled on whether its answer reports data, and filed on its own line.
    async fn run<B>(&self, selected: Selected, cut: Option<&RequestDeadline>, body: B) -> Response
    where
        B: Future<Output = Response> + Send,
    {
        let correlation = match nest_rs_core::__private::current_correlation() {
            Some(request) => request.child(),
            None => nest_rs_core::Correlation::minted(None),
        };
        let span = nest_rs_core::operation_span!(
            crate::unit::OPERATION,
            &correlation,
            "graphql.operation.type" = selected.type_name(),
            "graphql.operation.name" = selected.name.as_deref(),
        );
        let spec = UnitSpec::new(
            crate::unit::OPERATION,
            ExecutorScope::Request,
            selected.policy(),
        );
        let spec = match cut {
            Some(deadline) => spec.with_deadline(deadline.at()),
            None => spec,
        };
        let line = OperationLine::open(selected, correlation.clone(), span.clone());
        let body = async {
            let Some(deadline) = cut else {
                return Answered::Executed(body.await);
            };
            match tokio::time::timeout_at(deadline.at(), body).await {
                Ok(response) => Answered::Executed(response),
                Err(_) => {
                    line.cut(deadline.timeout());
                    Answered::Cut
                }
            }
        };
        let unit = run_unit(
            self.binding.as_ref(),
            spec,
            body,
            Answered::reports_data,
            Answered::unhonoured,
        );
        let scoped = nest_rs_core::with_request_scope(
            nest_rs_core::current_request_scope(),
            correlation,
            unit,
        );
        let answered = nest_rs_core::panic::contain(scoped).instrument(span).await;
        match answered {
            Ok(answered) => {
                line.file(answered.outcome());
                answered.into_response()
            }
            Err(unwound) => {
                line.file(PANIC);
                std::panic::resume_unwind(unwound)
            }
        }
    }
}

impl<E: Executor> Executor for OperationUnits<E> {
    async fn execute(&self, mut request: Request) -> Response {
        let selected = Selected::of(&mut request);
        self.execute_selected(selected, request, None).await
    }

    async fn execute_batch(&self, batch: BatchRequest) -> BatchResponse {
        self.execute_batch_within(batch, None).await
    }

    /// A graphql-ws operation: a query or a mutation runs as its unit at its
    /// start, inside the socket's; a subscription runs on the socket's own.
    fn execute_stream(
        &self,
        mut request: Request,
        session_data: Option<Arc<Data>>,
    ) -> BoxStream<'static, Response> {
        let selected = Selected::of(&mut request);
        if selected.ty == Some(OperationType::Subscription) {
            return self.executor.execute_stream(request, session_data);
        }
        let units = self.clone();
        stream::once(async move {
            let request = request.data(BatchSpan(tracing::Span::current()));
            let answer = async {
                units
                    .executor
                    .execute_stream(request, session_data)
                    .next()
                    .await
                    .unwrap_or_default()
            };
            units.run(selected, None, answer).await
        })
        .boxed()
    }
}

/// The type of the operation `request` selects, as async-graphql (`=7.2.1`)
/// selects it; `None` when it selects none and nothing runs.
pub(crate) fn selected_type(request: &mut Request) -> Option<OperationType> {
    Selected::of(request).ty
}

/// The operation `document` runs for a request naming `wanted`, as
/// async-graphql (`=7.2.1`) selects it, and the name the document declares it
/// under; `None` when it selects none and nothing runs.
pub(crate) fn selected_operation<'d>(
    document: &'d ExecutableDocument,
    wanted: Option<&str>,
) -> Option<(Option<&'d Name>, &'d Positioned<OperationDefinition>)> {
    match (&document.operations, wanted) {
        (DocumentOperations::Single(operation), None) => Some((None, operation)),
        (DocumentOperations::Multiple(operations), Some(wanted)) => operations
            .get_key_value(wanted)
            .map(|(name, operation)| (Some(name), operation)),
        (DocumentOperations::Multiple(operations), None) if operations.len() == 1 => operations
            .iter()
            .next()
            .map(|(name, operation)| (Some(name), operation)),
        _ => None,
    }
}

/// The operation a request selects, read off its document — parsed once,
/// since the request caches the parse for the executor.
struct Selected {
    ty: Option<OperationType>,
    /// The operation's name as its document declares it: a GraphQL name, never
    /// the free text a request's `operationName` may hold.
    name: Option<Name>,
}

impl Selected {
    const NONE: Self = Self {
        ty: None,
        name: None,
    };

    fn of(request: &mut Request) -> Self {
        let wanted = request.operation_name.take();
        let selected = request
            .parsed_query()
            .ok()
            .and_then(|document| selected_operation(document, wanted.as_deref()))
            .map_or(Self::NONE, |(name, operation)| Self {
                ty: Some(operation.node.ty),
                name: name.cloned(),
            });
        request.operation_name = wanted;
        selected
    }

    /// A mutation commits its work; anything else — a query, a document that
    /// selects nothing — reads, on the pool.
    fn commits(&self) -> bool {
        self.ty == Some(OperationType::Mutation)
    }

    fn policy(&self) -> TransactionPolicy {
        match self.commits() {
            true => TransactionPolicy::PerUnit,
            false => TransactionPolicy::Pool,
        }
    }

    fn type_name(&self) -> Option<&'static str> {
        self.ty.map(|ty| match ty {
            OperationType::Query => "query",
            OperationType::Mutation => "mutation",
            OperationType::Subscription => "subscription",
        })
    }
}

/// How an operation's unit ended, which decides how it settles.
#[expect(
    clippy::large_enum_variant,
    reason = "one per operation, moved to its settle and back: boxing would allocate where a move does not"
)]
enum Answered {
    /// It executed. Each root field that answered `null` for an error already
    /// rolled back alone, so the unit commits what the answer reports — unless
    /// it reports no data, a Non-Null root field's `null` having reached `data`.
    Executed(Response),
    /// The request's deadline cut it: nothing it ran commits.
    Cut,
    /// It executed and its unit could not honour it: nothing it wrote stands.
    Unhonoured,
}

impl Answered {
    /// Whether what it ran stands: an answer with no data reports none of it.
    fn reports_data(&self) -> bool {
        matches!(self, Self::Executed(response) if response.data != Value::Null)
    }

    fn unhonoured(_: Unhonoured) -> Self {
        Self::Unhonoured
    }

    fn outcome(&self) -> &'static str {
        match self {
            Self::Executed(response) if response.errors.is_empty() => OK,
            _ => ERROR,
        }
    }

    fn into_response(self) -> Response {
        match self {
            Self::Executed(response) => response,
            // Its unit said why (`OperationLine::cut`).
            Self::Cut => refused(&Problem::new(503, code::UNAVAILABLE)),
            // The driver filed why.
            Self::Unhonoured => refused(&Problem::new(500, code::INTERNAL)),
        }
    }
}

/// An operation answered `problem` alone: no data, since nothing it wrote
/// stands.
fn refused(problem: &Problem) -> Response {
    let error = crate::error::rendered(problem);
    let mut refused = ServerError::new(error.message, None);
    refused.extensions = error.extensions;
    Response::from_errors(vec![refused])
}

/// Resolve one mutation root field, its work settled on the answer it gives:
/// inside an atomic block of its operation's unit, rolled back when the field
/// answers `null` for an error — its own, or a Non-Null field's below it — and
/// released otherwise. None resolves once a root field before it nulled
/// `data`, since the operation then answers nothing. With no executor
/// installed — an app with no data layer — the field runs bare.
pub(crate) async fn resolve_mutation_field<F>(
    ctx: &Context<'_>,
    resolve: F,
) -> ServerResult<Option<Value>>
where
    F: Future<Output = ServerResult<Option<Value>>> + Send,
{
    let answer = ctx.data_opt::<MutationAnswer>();
    if answer.is_some_and(MutationAnswer::data_nulled) {
        return Ok(None);
    }
    // Exact: a mutation's root fields resolve one at a time, so no other field
    // adds an error meanwhile.
    let reported = errors_reported(ctx);
    if nest_rs_database::current_executor().is_none() {
        let resolved = resolve.await;
        answered(ctx, answer, &resolved, reported);
        return resolved;
    }
    let outcome = nest_rs_database::atomic(async {
        let resolved = resolve.await;
        match answered(ctx, answer, &resolved, reported) {
            RootAnswer::Stands => Ok(resolved),
            RootAnswer::Null | RootAnswer::NoData => Err(FieldFailed::Field(resolved)),
        }
    })
    .await;
    match outcome {
        Ok(resolved) | Err(FieldFailed::Field(resolved)) => resolved,
        Err(FieldFailed::Atomic(error)) => {
            let refused = Err(ctx.set_error_path(
                crate::error::opaque_error(&error).into_server_error(ctx.item.pos),
            ));
            // Its own `null` covers whatever its resolution filed below it.
            if let Some(answer) = answer {
                answer.root_field(ctx, &refused, &[]);
            }
            refused
        }
    }
}

/// What the root field `ctx` answers, given what it `resolved` and the errors
/// filed past the first `reported`: read off its operation's answer, or,
/// without one, `null` for any error.
fn answered(
    ctx: &Context<'_>,
    answer: Option<&MutationAnswer>,
    resolved: &ServerResult<Option<Value>>,
    reported: usize,
) -> RootAnswer {
    let errors = ctx
        .query_env
        .errors
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let filed = errors.get(reported..).unwrap_or_default();
    match answer {
        Some(answer) => answer.root_field(ctx, resolved, filed),
        None if resolved.is_err() || !filed.is_empty() => RootAnswer::Null,
        None => RootAnswer::Stands,
    }
}

/// How many errors the operation's response carries so far, a nested nullable
/// field's included: read where async-graphql (`=7.2.1`) adds them.
fn errors_reported(ctx: &Context<'_>) -> usize {
    ctx.query_env
        .errors
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len()
}

/// Why a root field's block rolls back.
enum FieldFailed {
    /// It answers `null` for an error, as the resolver wrote it.
    Field(ServerResult<Option<Value>>),
    /// The block could not be kept.
    Atomic(AtomicError),
}

impl From<AtomicError> for FieldFailed {
    fn from(error: AtomicError) -> Self {
        Self::Atomic(error)
    }
}

/// One operation's `graphql.operation` line, filed exactly once — by `Drop` as
/// [`CANCELLED`] when dropped first. It names the operation's type, never its
/// name: the document's, which the client writes, and which the span alone
/// carries, as the GraphQL conventions ask.
///
/// The correlation is held: a `Drop` runs while the future is torn down, not
/// reliably inside the scope that future installed.
struct OperationLine {
    ty: Option<&'static str>,
    correlation: nest_rs_core::Correlation,
    span: tracing::Span,
    started: std::time::Instant,
    filed: bool,
}

impl OperationLine {
    fn open(
        selected: Selected,
        correlation: nest_rs_core::Correlation,
        span: tracing::Span,
    ) -> Self {
        Self {
            ty: selected.type_name(),
            correlation,
            span,
            started: std::time::Instant::now(),
            filed: false,
        }
    }

    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
    }

    /// Said inside the operation's unit when the request's deadline cuts it,
    /// so each operation of a cut batch says it under its own ids.
    fn cut(&self, timeout: Duration) {
        tracing::warn!(
            target: crate::TARGET,
            operation_type = self.ty,
            ?timeout,
            "graphql operation not answered by the request deadline; it is cut",
        );
    }

    fn emit(&mut self, outcome: &'static str) {
        self.filed = true;
        nest_rs_core::RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            nest_rs_core::operation_line!(
                crate::unit::OPERATION,
                span: &self.span,
                outcome: outcome,
                started: self.started,
                operation_type = self.ty,
            );
        });
    }
}

impl Drop for OperationLine {
    fn drop(&mut self) {
        if !self.filed {
            self.emit(CANCELLED);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Mutex;

    use async_graphql::parser::types::Field;
    use async_graphql::registry::Registry;
    use async_graphql::{
        ContainerType, ContextSelectionSet, EmptySubscription, Object, ObjectType, OutputType,
        Schema,
    };

    use super::*;

    fn selected(query: &str, name: Option<&str>) -> Selected {
        let mut request = Request::new(query);
        if let Some(name) = name {
            request = request.operation_name(name);
        }
        Selected::of(&mut request)
    }

    #[test]
    fn a_query_reads_on_the_pool() {
        for query in [
            "query { me { id } }",
            "{ me { id } }",
            "{ __schema { types { name } } }",
            "{ mutationLog { id } }",
        ] {
            let read = selected(query, None);
            assert_eq!(read.ty, Some(OperationType::Query), "{query}");
            assert_eq!(read.policy(), TransactionPolicy::Pool, "{query}");
        }
    }

    #[test]
    fn a_mutation_commits_in_a_transaction_of_its_own() {
        let write = selected("mutation { createUser { id } }", None);
        assert!(write.commits());
        assert_eq!(write.policy(), TransactionPolicy::PerUnit);
        assert_eq!(write.type_name(), Some("mutation"));
    }

    #[test]
    fn the_operation_a_name_selects_is_the_one_read() {
        let document = "query Read { me { id } } mutation Write { createUser { id } }";
        let read = selected(document, Some("Read"));
        assert_eq!(read.ty, Some(OperationType::Query));
        assert_eq!(read.name.as_deref(), Some("Read"));
        assert!(selected(document, Some("Write")).commits());
        assert!(
            selected(document, None).ty.is_none(),
            "two operations and no name select none: nothing runs"
        );
        assert!(selected(document, Some("Missing")).ty.is_none());
    }

    #[test]
    fn a_lone_named_operation_is_selected_without_its_name() {
        let subscription = selected("subscription Ticks { ticks }", None);
        assert_eq!(subscription.ty, Some(OperationType::Subscription));
        assert_eq!(subscription.name.as_deref(), Some("Ticks"));
    }

    #[test]
    fn a_document_that_does_not_parse_selects_nothing_and_reads() {
        let unparsed = selected("mutation {{{", None);
        assert!(unparsed.ty.is_none());
        assert_eq!(unparsed.policy(), TransactionPolicy::Pool);
    }

    #[test]
    fn a_name_is_read_off_the_document_never_the_requests_free_text() {
        let unnamed = selected("{ me { id } }", Some("drop table users"));
        assert!(
            unnamed.ty.is_none(),
            "async-graphql refuses a name on an anonymous operation"
        );
        assert!(unnamed.name.is_none());
    }

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
        async fn shaped(&self) -> Shape {
            Shape
        }
    }

    struct Shape;

    #[Object]
    impl Shape {
        async fn broken(&self) -> async_graphql::Result<Option<i32>> {
            Err("the nested field failed".into())
        }
    }

    /// The mutation root, counting the errors each root field's resolution adds.
    struct Counted {
        root: Mutation,
        added: Arc<Mutex<Vec<usize>>>,
    }

    impl OutputType for Counted {
        fn type_name() -> Cow<'static, str> {
            <Mutation as OutputType>::type_name()
        }

        fn create_type_info(registry: &mut Registry) -> String {
            <Mutation as OutputType>::create_type_info(registry)
        }

        async fn resolve(
            &self,
            _ctx: &ContextSelectionSet<'_>,
            _field: &Positioned<Field>,
        ) -> ServerResult<Value> {
            unreachable!("a root resolves through resolve_field")
        }
    }

    impl ContainerType for Counted {
        async fn resolve_field(&self, ctx: &Context<'_>) -> ServerResult<Option<Value>> {
            let before = errors_reported(ctx);
            let resolved = self.root.resolve_field(ctx).await;
            self.added
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(errors_reported(ctx) - before);
            resolved
        }
    }

    impl ObjectType for Counted {}

    /// Pins where async-graphql adds a nested nullable field's error: a bump
    /// that moves it fails here, not in a response that commits a failed field.
    #[tokio::test]
    async fn the_counter_moves_on_a_nested_nullable_error() {
        let added = Arc::new(Mutex::new(Vec::new()));
        let schema = Schema::new(
            Query,
            Counted {
                root: Mutation,
                added: Arc::clone(&added),
            },
            EmptySubscription,
        );
        let response = schema.execute("mutation { shaped { broken } }").await;
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(*added.lock().unwrap(), [1]);
    }
}

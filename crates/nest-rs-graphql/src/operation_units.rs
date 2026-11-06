//! Every operation a GraphQL request carries runs as a unit of work of its
//! own, [`OPERATION`](crate::unit::OPERATION), through the binding the mount
//! resolves: a query on the pool, a mutation in one transaction whose root
//! fields each run in an atomic block settled on the field's own answer, so a
//! field that answers `null` for an error rolls back alone while the
//! operation's other fields commit, and an operation whose answer reports no
//! data rolls back whole — a response never reports work that was rolled back,
//! nor stays silent on work that stands.

use std::collections::{HashMap, HashSet};
use std::convert::identity;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, PoisonError};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use async_graphql::futures_util::StreamExt;
use async_graphql::futures_util::stream::{self, BoxStream, FuturesOrdered, Stream};
use async_graphql::parser::types::{
    Directive, DocumentOperations, ExecutableDocument, FragmentDefinition, OperationDefinition,
    OperationType, Selection, SelectionSet, VariableDefinition,
};
use async_graphql::{
    BatchRequest, BatchResponse, Context, Data, Executor, Name, Pos, Positioned, Request, Response,
    ServerError, ServerResult, Value, Variables,
};
use nest_rs_core::__private::{RequestScopes, nested_continuation};
use nest_rs_core::operation_log::{CANCELLED, ERROR, OK, PANIC};
use nest_rs_core::{Problem, code};
use nest_rs_core::{RequestContinuation, RequestScope};
use nest_rs_database::{
    AtomicError, ExecutorScope, TransactionPolicy, Unhonoured, UnitOfWork, UnitSpec, run_unit,
};
use nest_rs_http::__private::RequestDeadline;
use tracing::Instrument;

use crate::error::AnsweredError;
use crate::loader::BatchSpan;
use crate::null_propagation::{HiddenAnswers, MutationAnswer, RootAnswer};

/// The schema `E`, each query and mutation it executes run as a unit of work
/// of its own; a subscription's unit is its connection.
#[derive(Clone)]
pub(crate) struct OperationUnits<E> {
    executor: E,
    /// The unit-of-work binding, resolved once at mount; with none, every
    /// operation runs bare.
    binding: Option<Arc<dyn UnitOfWork>>,
    /// Where a graphql-ws operation's request scope comes from: its socket
    /// carries none of its upgrade's.
    scopes: RequestScopes,
}

impl<E: Executor> OperationUnits<E> {
    pub(crate) fn new(
        executor: E,
        binding: Option<Arc<dyn UnitOfWork>>,
        scopes: RequestScopes,
    ) -> Self {
        Self {
            executor,
            binding,
            scopes,
        }
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
        let body = self.executor.execute(request);
        let scope = nest_rs_core::current_request_scope();
        self.run(selected, scope, cut, body).await
    }

    /// Run `body`, the operation `selected` names, as its unit in `scope`: cut
    /// at `cut`, settled on whether its answer reports data, and filed on its
    /// own line.
    async fn run<B>(
        &self,
        selected: Selected,
        scope: Option<Arc<RequestScope>>,
        cut: Option<&RequestDeadline>,
        body: B,
    ) -> Response
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
        let scoped = nest_rs_core::with_request_scope(scope, correlation, unit);
        let answered = nest_rs_core::panic::contain(scoped).instrument(span).await;
        match answered {
            Ok(answered) => {
                line.answered(&answered);
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

    /// A graphql-ws operation, in a request scope of its own: a query or a
    /// mutation runs as its unit at its start, inside the socket's; a
    /// subscription runs on the socket's own, its opening and every item in
    /// the scope it opened with.
    fn execute_stream(
        &self,
        mut request: Request,
        session_data: Option<Arc<Data>>,
    ) -> BoxStream<'static, Response> {
        let selected = Selected::of(&mut request);
        let scope = self.scopes.open();
        if selected.ty == Some(OperationType::Subscription) {
            return InScope {
                scope: Some(scope),
                unit: None,
                stream: self.executor.execute_stream(request, session_data),
            }
            .boxed();
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
            units.run(selected, Some(scope), None, answer).await
        })
        .boxed()
    }
}

/// A subscription's stream, each poll of it in the request scope the
/// subscription opened with, nested in the socket's unit, which polls it.
struct InScope {
    /// Taken into [`unit`](Self::unit) at the first poll.
    scope: Option<Arc<RequestScope>>,
    unit: Option<RequestContinuation>,
    stream: BoxStream<'static, Response>,
}

impl Stream for InScope {
    type Item = Response;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Response>> {
        let this = self.get_mut();
        let unit = this.unit.get_or_insert_with(|| {
            nested_continuation(this.scope.take(), nest_rs_core::Correlation::inherited())
        });
        unit.enter(|| this.stream.poll_next_unpin(cx))
    }
}

/// The type of the operation `request` selects, as async-graphql (`7.2.1`)
/// selects it; `None` when it selects none and nothing runs.
pub(crate) fn selected_type(request: &mut Request) -> Option<OperationType> {
    read_selected(request, |_, _, operation| operation.node.ty)
}

/// What `read` makes of the operation `request` selects, its document parsed
/// once, since the request caches the parse for the executor; `None` when it
/// selects none and nothing runs.
pub(crate) fn read_selected<T>(
    request: &mut Request,
    read: impl FnOnce(&ExecutableDocument, Option<&Name>, &Positioned<OperationDefinition>) -> T,
) -> Option<T> {
    let wanted = request.operation_name.take();
    let read = request.parsed_query().ok().and_then(|document| {
        let (name, operation) = selected_operation(document, wanted.as_deref())?;
        Some(read(document, name, operation))
    });
    request.operation_name = wanted;
    read
}

/// The operation `document` runs for a request naming `wanted`, as
/// async-graphql (`7.2.1`) selects it, and the name the document declares it
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

/// What an operation answers in place of running, said once validation passed
/// ([`RequestExtension`](crate::request_extension)): a document validation
/// refuses says what validation says of it.
pub(crate) enum OperationRefusal {
    /// Read off the document before the executor.
    Read(ServerError),
    /// Still to read, once validation bounded the walk it takes.
    Unread(HiddenAnswers),
}

impl OperationRefusal {
    /// What it answers, when it answers anything.
    pub(crate) fn said(&self) -> Option<ServerError> {
        match self {
            Self::Read(refusal) => Some(refusal.clone()),
            Self::Unread(hidden) => hidden.refusal(),
        }
    }
}

/// The deepest selection set async-graphql (`7.2.1`, `check_recursive_depth`,
/// `limit_recursive_depth`'s default) runs, nesting through fragments included,
/// and refuses every document past. A read of a document before that check
/// stops here: a fragment chain as long as a request's body would otherwise
/// outgrow the stack, which aborts the process.
pub(crate) const RECURSIVE_DEPTH: usize = 32;

/// The operation a request selects, read off its document; nothing when it
/// selects none.
#[derive(Default)]
struct Selected {
    ty: Option<OperationType>,
    /// The operation's name as its document declares it: a GraphQL name, never
    /// the free text a request's `operationName` may hold.
    name: Option<Name>,
}

impl Selected {
    /// Read the operation `request` selects, its variables coerced for what
    /// reads them next, and any [`OperationRefusal`] left in its data. After a
    /// global pipe, which is handed the variables as the request sent them.
    fn of(request: &mut Request) -> Self {
        let mut variables = std::mem::take(&mut request.variables);
        let read = read_selected(request, |document, name, operation| {
            let operation = &operation.node;
            let refusal = coerce_variables(&operation.variable_definitions, &mut variables)
                .or_else(|| match operation.ty {
                    OperationType::Mutation => repeated_root_selection(
                        &document.fragments,
                        &operation.selection_set.node,
                        &variables,
                    ),
                    OperationType::Query | OperationType::Subscription => None,
                });
            let selected = Self {
                ty: Some(operation.ty),
                name: name.cloned(),
            };
            (selected, refusal)
        });
        request.variables = variables;
        let (selected, refusal) = read.unwrap_or_default();
        if let Some(refusal) = refusal {
            request.data.insert(OperationRefusal::Read(refusal));
        }
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

    /// Whether it failed by what its caller's document earned: errors, none of
    /// them a failure of the server's own.
    fn refused_its_caller(&self) -> bool {
        matches!(self, Self::Executed(response)
            if !response.errors.is_empty()
                && !response.errors.iter().any(AnsweredError::failed_the_server))
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

/// GraphQL §6.1.2, *CoerceVariableValues*, for the absent variables
/// async-graphql (`7.2.1`) leaves uncoerced: each takes its default, which
/// `@skip` and `@include` read in `request.variables` alone (`schema.rs`,
/// `remove_skipped_selection`) where an argument reads the definition too. The
/// first Non-Null one with none is the request error it returns: async-graphql
/// runs such an operation, reading the directive's `if` as `false` and failing
/// only the fields whose arguments read it, after the root fields before them.
fn coerce_variables(
    definitions: &[Positioned<VariableDefinition>],
    variables: &mut Variables,
) -> Option<ServerError> {
    let mut absent = None;
    for definition in definitions {
        let name = &definition.node.name.node;
        if variables.contains_key(name) {
            continue;
        }
        match &definition.node.default_value {
            Some(default) => {
                variables.insert(name.clone(), default.node.clone());
            }
            None if !definition.node.var_type.node.nullable => {
                absent.get_or_insert(definition);
            }
            None => {}
        }
    }
    absent.map(|definition| {
        let said = format!(
            r#"Variable "${}" of required type "{}" was not provided."#,
            definition.node.name.node, definition.node.var_type.node,
        );
        crate::error::invalid_argument(said).into_server_error(definition.pos)
    })
}

/// Whether `directives` leave their selection out, as async-graphql (`7.2.1`)
/// decides it where the rule is private (`schema.rs`,
/// `remove_skipped_selection`): `@skip` and `@include`, each `if` read in
/// `variables` alone, and anything but a boolean reading `false`. Pinned by
/// `a_selection_is_left_out_as_async_graphql_leaves_it_out`.
pub(crate) fn is_skipped(directives: &[Positioned<Directive>], variables: &Variables) -> bool {
    directives.iter().any(|directive| {
        let include = match directive.node.name.node.as_str() {
            "skip" => false,
            "include" => true,
            _ => return false,
        };
        directive.node.get_argument("if").is_some_and(|condition| {
            let value = condition
                .node
                .clone()
                .into_const_with(|name| variables.get(&name).cloned().ok_or(()));
            include != matches!(value, Ok(Value::Boolean(true)))
        })
    })
}

/// The refusal of a mutation whose root selections, through its fragments,
/// name one response key or spread one fragment more than once, pointing at
/// the repeated selection: async-graphql (`7.2.1`) runs a root field once per
/// selection — its writes twice — where GraphQL §6.3.2 runs one key once, and
/// answers only one of the two. A selection `@skip` or `@include` leaves out
/// runs nothing, so it repeats nothing.
fn repeated_root_selection(
    fragments: &HashMap<Name, Positioned<FragmentDefinition>>,
    root: &SelectionSet,
    variables: &Variables,
) -> Option<ServerError> {
    let mut selections = RootSelections {
        fragments,
        variables,
        keys: HashSet::new(),
        spread: HashSet::new(),
        expanding: Vec::new(),
    };
    let (repeated, at) = selections.first_repeated(root, 0)?;
    let said = match repeated {
        Repeated::Key(key) => format!(r#"the mutation selects "{key}" more than once at its root"#),
        Repeated::Fragment(name) => {
            format!(r#"the mutation spreads "{name}" more than once at its root"#)
        }
    };
    Some(crate::error::invalid_argument(said).into_server_error(at))
}

/// A mutation's root selections, read before validation: each fragment is
/// expanded once at most and no deeper than [`RECURSIVE_DEPTH`], so the walk
/// never outgrows the document or the stack, and a fragment cycle, which
/// validation refuses, ends it.
struct RootSelections<'d> {
    fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
    variables: &'d Variables,
    keys: HashSet<&'d str>,
    spread: HashSet<&'d str>,
    /// The fragments being expanded, the innermost last.
    expanding: Vec<&'d str>,
}

/// What a root selection names a second time.
enum Repeated<'d> {
    Key(&'d str),
    Fragment(&'d str),
}

impl<'d> RootSelections<'d> {
    /// The first selection of `set`, `depth` selection sets below the root,
    /// naming a response key, or spreading a fragment, the root already holds, and
    /// where it stands.
    fn first_repeated(
        &mut self,
        set: &'d SelectionSet,
        depth: usize,
    ) -> Option<(Repeated<'d>, Pos)> {
        if depth > RECURSIVE_DEPTH {
            return None;
        }
        let variables = self.variables;
        set.items
            .iter()
            .filter(|selection| !is_skipped(selection.node.directives(), variables))
            .find_map(|selection| match &selection.node {
                Selection::Field(field) => {
                    let key = field.node.response_key().node.as_str();
                    (!self.keys.insert(key)).then_some((Repeated::Key(key), selection.pos))
                }
                Selection::FragmentSpread(spread) => {
                    let name = spread.node.fragment_name.node.as_str();
                    if self.expanding.contains(&name) {
                        return None;
                    }
                    if !self.spread.insert(name) {
                        return Some((Repeated::Fragment(name), selection.pos));
                    }
                    let fragment = self.fragments.get(name)?;
                    self.expanding.push(name);
                    let repeated =
                        self.first_repeated(&fragment.node.selection_set.node, depth + 1);
                    self.expanding.pop();
                    repeated
                }
                Selection::InlineFragment(fragment) => {
                    self.first_repeated(&fragment.node.selection_set.node, depth + 1)
                }
            })
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
/// field's included: read where async-graphql (`7.2.1`) adds them.
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

    /// File what the operation answered: `ok`, or `error`, which its span
    /// records unless its caller's document earned it, as an HTTP `4xx` fails
    /// nothing on the server's span.
    fn answered(mut self, answered: &Answered) {
        if answered.refused_its_caller() {
            self.span = tracing::Span::none();
        }
        self.emit(answered.outcome());
    }

    /// Said inside the operation's unit when the request's deadline cuts it,
    /// so each operation of a cut batch says it under its own ids.
    fn cut(&self, timeout: Duration) {
        tracing::warn!(
            target: crate::TARGET,
            kind = self.ty,
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
                kind = self.ty,
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

    fn request(query: &str, name: Option<&str>) -> Request {
        let request = Request::new(query);
        match name {
            Some(name) => request.operation_name(name),
            None => request,
        }
    }

    fn selected(query: &str, name: Option<&str>) -> Selected {
        Selected::of(&mut request(query, name))
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

    /// What a read of `request` leaves it answering in place of running.
    fn refused(mut request: Request) -> Option<ServerError> {
        Selected::of(&mut request);
        request
            .data
            .get(&std::any::TypeId::of::<OperationRefusal>())
            .and_then(|held| held.downcast_ref::<OperationRefusal>())
            .and_then(OperationRefusal::said)
    }

    /// What a request is refused before it runs, when it is.
    fn refusal(query: &str, name: Option<&str>) -> Option<String> {
        refused(request(query, name)).map(|refusal| refusal.message)
    }

    #[test]
    fn a_mutation_naming_one_root_field_twice_is_refused_naming_it() {
        let selects =
            |key: &str| format!(r#"the mutation selects "{key}" more than once at its root"#);
        let spreads =
            |name: &str| format!(r#"the mutation spreads "{name}" more than once at its root"#);
        for (query, said) in [
            ("mutation { a a }", selects("a")),
            ("mutation { x: a x: b }", selects("x")),
            ("mutation { a b: a a }", selects("a")),
            ("mutation { a ... { a } }", selects("a")),
            ("mutation { a ... on Mutation { a } }", selects("a")),
            (
                "mutation { ...A b } fragment A on Mutation { b }",
                selects("b"),
            ),
            (
                "mutation { ...A ...B } fragment A on Mutation { a } fragment B on Mutation { a }",
                selects("a"),
            ),
            (
                "mutation { ...A ...A } fragment A on Mutation { a }",
                spreads("A"),
            ),
            (
                "mutation { ...A ...B } fragment A on Mutation { ...C } \
                 fragment B on Mutation { ...C } fragment C on Mutation { c }",
                spreads("C"),
            ),
        ] {
            assert_eq!(refusal(query, None), Some(said), "{query}");
        }
        assert_eq!(
            refusal("query Q { a } mutation M { a a }", Some("M")),
            Some(selects("a"))
        );
    }

    #[test]
    fn a_refusal_points_at_the_selection_repeated_and_says_its_class() {
        let Some(refusal) = refused(request("mutation {\n  a\n  a\n}", None)) else {
            panic!("the mutation names `a` twice");
        };
        assert_eq!(refusal.locations, [Pos { line: 3, column: 3 }]);
        assert!(refusal.path.is_empty(), "the operation's, not a field's");
        let code = refusal
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get(crate::error::CODE_EXTENSION));
        assert_eq!(
            code,
            Some(&Value::from(code::INVALID_ARGUMENT.as_str())),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_document_naming_each_root_field_once_is_not_refused() {
        for (query, name) in [
            ("mutation { a b }", None),
            ("mutation { a { b b } c: a { b } }", None),
            (
                "mutation { ...A b } fragment A on Mutation { a { x x } }",
                None,
            ),
            ("{ a a }", None),
            ("query Q { a a } mutation M { a }", Some("M")),
            // Validation refuses these; the walk ends on them.
            ("mutation { ...A } fragment A on Mutation { ...A b }", None),
            ("mutation { ...Missing a }", None),
            ("mutation { a a", None),
        ] {
            assert_eq!(refusal(query, name), None, "{query}");
        }
    }

    /// `query`'s variables once read, sent as `variables`.
    fn coerced(query: &str, variables: serde_json::Value) -> serde_json::Value {
        let mut request = Request::new(query).variables(Variables::from_json(variables));
        Selected::of(&mut request);
        request.variables.into_value().into_json().unwrap()
    }

    #[test]
    fn an_absent_variable_takes_its_default_and_a_sent_one_keeps_its_value() {
        use serde_json::json;
        for (query, sent, read) in [
            (
                "query($a: Boolean = true) { x }",
                json!({}),
                json!({ "a": true }),
            ),
            (
                "query($a: Boolean = true) { x }",
                json!({ "a": false }),
                json!({ "a": false }),
            ),
            (
                "query($a: Boolean = true) { x }",
                json!({ "a": null }),
                json!({ "a": null }),
            ),
            ("query($a: Int) { x }", json!({}), json!({})),
            (
                "query($a: Int = 1, $b: [String!] = [\"c\"], $d: Int) { x }",
                json!({}),
                json!({ "a": 1, "b": ["c"] }),
            ),
            (
                "query A($a: Int = 1) { x } query B($b: Int = 2) { x }",
                json!({}),
                json!({}),
            ),
        ] {
            assert_eq!(coerced(query, sent), read, "{query}");
        }
    }

    #[test]
    fn the_operation_a_name_selects_gives_its_own_defaults() {
        let mut request = Request::new("query A($a: Int = 1) { x } query B($a: Int = 2) { x }")
            .operation_name("B");
        Selected::of(&mut request);
        assert_eq!(
            request.variables.into_value().into_json().unwrap(),
            serde_json::json!({ "a": 2 })
        );
    }

    #[test]
    fn a_required_variable_absent_refuses_the_operation_naming_it_and_where_it_is_declared() {
        let Some(refusal) = refused(request(
            "query(\n  $a: Int = 1,\n  $b: [Int!]!\n) { x }",
            None,
        )) else {
            panic!("`$b` is required and absent");
        };
        assert_eq!(
            refusal.message,
            r#"Variable "$b" of required type "[Int!]!" was not provided."#
        );
        assert_eq!(refusal.locations, [Pos { line: 3, column: 3 }]);
        let code = refusal
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get(crate::error::CODE_EXTENSION));
        assert_eq!(code, Some(&Value::from(code::INVALID_ARGUMENT.as_str())));
        assert!(
            refused(
                request("query($b: Int!) { x }", None)
                    .variables(Variables::from_json(serde_json::json!({ "b": 1 })))
            )
            .is_none(),
            "sent"
        );
        assert!(
            refused(request("query($b: Int! = 1) { x }", None)).is_none(),
            "its default"
        );
        assert!(refused(request("query($b: Int) { x }", None)).is_none());
        assert!(
            refused(request("query A($b: Int!) { x } query B { x }", Some("B"))).is_none(),
            "another operation's"
        );
    }

    #[test]
    fn a_selection_a_directive_leaves_out_repeats_nothing() {
        let with = |query: &str, variables: serde_json::Value| {
            refused(request(query, None).variables(Variables::from_json(variables)))
                .map(|refusal| refusal.message)
        };
        for (query, variables) in [
            ("mutation { a @skip(if: true) a }", serde_json::json!({})),
            (
                "mutation { a a @include(if: false) }",
                serde_json::json!({}),
            ),
            (
                "mutation($x: Boolean!) { a @skip(if: $x) a @include(if: $x) }",
                serde_json::json!({ "x": true }),
            ),
            (
                "mutation($x: Boolean!) { a @skip(if: $x) a @include(if: $x) }",
                serde_json::json!({ "x": false }),
            ),
            (
                "mutation($x: Boolean = true) { a @skip(if: $x) a }",
                serde_json::json!({}),
            ),
            (
                "mutation { ...A @skip(if: true) ...A } fragment A on Mutation { a }",
                serde_json::json!({}),
            ),
            (
                "mutation { a ... @skip(if: true) { a } }",
                serde_json::json!({}),
            ),
            (
                "mutation { a ...A } fragment A on Mutation { a @skip(if: true) }",
                serde_json::json!({}),
            ),
        ] {
            assert_eq!(with(query, variables), None, "{query}");
        }
        for (query, variables, key) in [
            (
                "mutation { a @skip(if: false) a }",
                serde_json::json!({}),
                "a",
            ),
            (
                "mutation { a a @include(if: true) }",
                serde_json::json!({}),
                "a",
            ),
            (
                "mutation($x: Boolean = false) { a @skip(if: $x) a }",
                serde_json::json!({}),
                "a",
            ),
            (
                "mutation($x: Boolean = false) { a @skip(if: $x) a }",
                serde_json::json!({ "x": false }),
                "a",
            ),
        ] {
            assert_eq!(
                with(query, variables),
                Some(format!(
                    r#"the mutation selects "{key}" more than once at its root"#
                )),
                "{query}"
            );
        }
    }

    struct Query;

    #[Object]
    impl Query {
        async fn up(&self) -> bool {
            true
        }
    }

    /// [`is_skipped`] copies a private rule of async-graphql's: a bump that
    /// moves the rule fails here, not in a mutation refused or an answer placed
    /// by another reading of the document than the one that ran.
    #[tokio::test]
    async fn a_selection_is_left_out_as_async_graphql_leaves_it_out() {
        let schema = Schema::new(Query, async_graphql::EmptyMutation, EmptySubscription);
        for directives in [
            "",
            "@skip(if: true)",
            "@skip(if: false)",
            "@include(if: true)",
            "@include(if: false)",
            "@skip(if: true) @include(if: true)",
            "@skip(if: false) @include(if: false)",
            "@skip(if: false) @include(if: true)",
            "@include(if: false) @skip(if: true)",
        ] {
            let query = format!("{{ up {directives} kept: up }}");
            assert_runs_as_read(&schema, &query, serde_json::json!({})).await;
        }
        for directives in ["@skip(if: $x)", "@include(if: $x)"] {
            let query = format!("query($x: Boolean!) {{ up {directives} kept: up }}");
            for sent in [
                serde_json::json!({ "x": true }),
                serde_json::json!({ "x": false }),
                serde_json::json!({}),
            ] {
                assert_runs_as_read(&schema, &query, sent).await;
            }
        }
    }

    /// `query`, its `up` selection read by [`is_skipped`], and what the schema
    /// itself ran of it, agree.
    async fn assert_runs_as_read(
        schema: &Schema<Query, async_graphql::EmptyMutation, EmptySubscription>,
        query: &str,
        sent: serde_json::Value,
    ) {
        let variables = Variables::from_json(sent);
        let document = async_graphql::parser::parse_query(query).unwrap();
        let Some((_, operation)) = selected_operation(&document, None) else {
            panic!("{query} selects its operation");
        };
        let Selection::Field(up) = &operation.node.selection_set.node.items[0].node else {
            panic!("{query} opens on `up`");
        };
        let read = is_skipped(&up.node.directives, &variables);
        let response = schema
            .execute(Request::new(query).variables(variables))
            .await;
        assert!(response.errors.is_empty(), "{query}: {:?}", response.errors);
        let Value::Object(answered) = &response.data else {
            panic!("{query}: `kept` answers");
        };
        assert_eq!(read, !answered.contains_key("up"), "{query}");
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

    /// Pins why a mutation naming one root field twice is refused: async-graphql
    /// runs it once per selection, where GraphQL §6.3.2 runs one response key
    /// once. A bump that merges them fails here, and the refusal can go.
    #[tokio::test]
    async fn a_root_field_selected_twice_runs_twice() {
        let added = Arc::new(Mutex::new(Vec::new()));
        let schema = Schema::new(
            Query,
            Counted {
                root: Mutation,
                added: Arc::clone(&added),
            },
            EmptySubscription,
        );
        schema
            .execute("mutation { a: shaped { broken } a: shaped { broken } }")
            .await;
        assert_eq!(*added.lock().unwrap(), [1, 1]);
    }
}

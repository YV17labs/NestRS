//! The wrapper that runs each MCP operation as a unit of its own.
//!
//! rmcp dispatches every operation on its own task, where the request's
//! context is gone; [`PropagatingHandler`] re-installs it inside the dispatch,
//! then runs a request as one `mcp.operation` unit on the pipeline: the
//! request's own site (the unit stage), inside a unit of work, around rmcp's
//! routing — where a `#[tools]` operation runs its own site. A method it does
//! not delegate silently reverts to rmcp's default, so the impl carries
//! `#[deny(clippy::missing_trait_methods)]`.
//!
//! | Kind | Request context | Unit stage | Unit of work | Deadline | Files a line |
//! |---|---|---|---|---|---|
//! | Requests (`tools/call`, `prompts/get`, `resources/read`, `completion/complete`, `logging/setLevel`, `tasks/*`, lifecycle, custom) | yes | yes | yes | yes | yes |
//! | `subscriptions/listen` | yes | no | no — it outlives any sane transaction | no | yes |
//! | Notifications | yes | no | no — nothing to commit or roll back on | no | yes |
//! | Synchronous accessors (`get_info`, `get_tool`, …) | — | — | — | — | no — no work is dispatched |
//!
//! The request context is the `/mcp` request's: its span, scope and
//! correlation, and what its guards and layers made ambient.
//!
//! An operation files `ok` or `error` — `error` too when it outlives
//! `McpConfig::operation_timeout`, which every request but `subscriptions/listen`
//! runs under; `panic` when it unwinds (contained, the client answered with an
//! internal error rather than left to its timeout); or `cancelled` when its
//! client cancels it or the transport stops it. The line is filed by
//! [`OperationLine`], so an end nobody saw still files one.

#![expect(
    deprecated,
    reason = "rmcp still routes the deprecated methods for legacy protocol versions"
)]

use std::any::Any;
use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::future::Either;
use nest_rs_core::__private::{Tail, UnitChains, dispatch, new_unit_context, settled};
use nest_rs_core::{
    Correlation, Deadline, Failure, Outcome, RequestContinuation, UnitContext, operation_log,
};
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_http::DetachedWork;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResponse, CallToolResult,
    CancelTaskMethod, CancelTaskParams, CancelledNotificationMethod, CancelledNotificationParam,
    CompleteRequestMethod, CompleteRequestParams, CompleteResult, ConstString, CustomNotification,
    CustomRequest, CustomResult, DiscoverRequestMethod, DiscoverResult, GetPromptRequestMethod,
    GetPromptRequestParams, GetPromptResponse, GetTaskMethod, GetTaskParams, GetTaskResult,
    InitializeRequestParams, InitializeResult, InitializeResultMethod,
    InitializedNotificationMethod, ListPromptsRequestMethod, ListPromptsResult,
    ListResourceTemplatesRequestMethod, ListResourceTemplatesResult, ListResourcesRequestMethod,
    ListResourcesResult, ListToolsRequestMethod, ListToolsResult, PaginatedRequestParams,
    PingRequestMethod, ProgressNotificationMethod, ProgressNotificationParam, ProtocolVersion,
    ReadResourceRequestMethod, ReadResourceRequestParams, ReadResourceResponse, Reference,
    RootsListChangedNotificationMethod, ServerConfig, SetLevelRequestMethod, SetLevelRequestParams,
    SubscribeRequestMethod, SubscribeRequestParams, SubscriptionFilter,
    SubscriptionsListenRequestMethod, Tool, UnsubscribeRequestMethod, UnsubscribeRequestParams,
    UpdateTaskMethod, UpdateTaskParams,
};
use rmcp::service::{
    MaybeSendFuture, NotificationContext, RequestContext, RoleServer, SubscriptionContext,
};
use tracing::Instrument;

use crate::McpError;
use crate::ambient::McpAmbient;
use crate::error::{
    ToolReportedError, answer, method_not_found, opaque_answer, prompt_not_found, refuses_method,
    refuses_prompt, refuses_task, task_not_found, tool_not_found,
};
use crate::guard::BoxFuture;
use crate::operation::McpOperationKind;
use crate::registry::MountIndex;
use crate::view::McpView;

/// The chains every request's own site runs on one mount.
#[derive(Clone)]
pub(crate) enum RequestChains {
    /// A hand-built endpoint's, which no app composed: its requests run their
    /// tail alone.
    Bare,
    /// The chains the mount's container composed.
    Composed(Arc<UnitChains>),
    /// The mount's container refused them, and said why: every request is
    /// refused opaquely.
    Refused,
}

/// Wraps a tool host so each operation runs with the `/mcp` request's context
/// installed, as a unit of its own on the pipeline.
///
/// Built by [`endpoint`](crate::__private::endpoint), never by hand.
pub struct PropagatingHandler<H> {
    inner: H,
    units: OperationUnits,
}

impl<H> PropagatingHandler<H> {
    pub(crate) fn new(inner: H, units: OperationUnits) -> Self {
        Self { inner, units }
    }

    /// Run `inner` as one operation of the mount — see
    /// [`OperationUnits::dispatch`] — its line naming a tool the mount reads
    /// declared, or the host's `get_tool` knows where the mount cannot say.
    async fn dispatch<T, F, S>(
        &self,
        method: &Method,
        addressed: &Addressed,
        ambient: Option<McpAmbient>,
        unit: bool,
        stopped: S,
        inner: F,
    ) -> Result<T, McpError>
    where
        H: ServerHandler,
        T: Send + 'static,
        F: Future<Output = Result<T, McpError>> + Send,
        S: Future<Output = Result<T, McpError>> + Send,
    {
        let declared = addressed.declared(&self.inner, self.units.index.as_deref());
        self.units
            .dispatch(method, addressed, declared, ambient, unit, stopped, inner)
            .await
    }
}

/// What one mount runs each request as: an operation unit — its own site, a
/// unit of work, a deadline and a stop. Every session's handler shares it, and
/// so does the endpoint, which answers a call naming no declared tool before
/// rmcp sees it.
#[derive(Clone)]
pub(crate) struct OperationUnits {
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    detached: DetachedWork,
    /// What one request operation may run for; `None` ⇒ no deadline.
    operation_timeout: Option<Duration>,
    /// What the mount's hosts declare; `None` asks the host's `get_tool`.
    index: Option<Arc<MountIndex>>,
    chains: RequestChains,
}

impl OperationUnits {
    pub(crate) fn new(
        unit_of_work: Option<Arc<dyn UnitOfWork>>,
        detached: DetachedWork,
        operation_timeout: Option<Duration>,
        index: Option<Arc<MountIndex>>,
        chains: RequestChains,
    ) -> Self {
        Self {
            unit_of_work,
            detached,
            operation_timeout,
            index,
            chains,
        }
    }

    /// Answer a `tools/call` naming `name`, a tool no host on the mount
    /// declares, as the operation the handler would have run: its unit stage,
    /// its line `error` and the refusal rmcp's own handler answers,
    /// `tool not found`, unless a layer around it answered otherwise.
    pub(crate) async fn undeclared_call(&self, name: String) -> McpError {
        let method = Method::from(CallToolRequestMethod::VALUE);
        let addressed = Addressed::Tool(name);
        let answered = self
            .dispatch::<std::convert::Infallible, _, _>(
                &method,
                &addressed,
                Some(operation_log::OTHER),
                Some(McpAmbient::capture()),
                true,
                std::future::pending(),
                async { Err(tool_not_found()) },
            )
            .await;
        let Err(error) = answered;
        error
    }

    /// Run `inner` as one operation, under the `/mcp` request's context
    /// `ambient` carries.
    ///
    /// `method` is the JSON-RPC method and `addressed` what it named, which the
    /// span and the view record as the conventions ask; the line names
    /// `declared` alone, a tool the host declares under that name. `unit` is
    /// `false` for an operation that runs neither the request's site, a unit
    /// of work nor the deadline, which cuts the body, never its settle, so a
    /// commit is never cut. `stopped` resolves when the operation is to end
    /// before it answers, with what the client is answered; like the deadline
    /// it cuts the body alone, so a stop arriving while the unit settles leaves
    /// the operation to end as it answered.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is one thing the caller read off the request; a struct would only rename them"
    )]
    async fn dispatch<T, F, S>(
        &self,
        method: &Method,
        addressed: &Addressed,
        declared: Option<&str>,
        ambient: Option<McpAmbient>,
        unit: bool,
        stopped: S,
        inner: F,
    ) -> Result<T, McpError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, McpError>> + Send,
        S: Future<Output = Result<T, McpError>> + Send,
    {
        // Boxed, as rmcp's own future is: rmcp holds every method's future in
        // its loop, so each stays a pointer there whatever the operation runs.
        let run: BoxFuture<'_, Result<T, McpError>> =
            Box::pin(self.operation(method, addressed, declared, unit, stopped, inner));
        match ambient {
            Some(ambient) => ambient.carried.carry(run).await,
            None => run.await,
        }
    }

    /// [`dispatch`](Self::dispatch)'s operation, inside whatever context it
    /// installed: the nesting every operation shares, outermost first — its
    /// span and line, the request scope, the unit of work (the transaction),
    /// the request's own site, rmcp's routing. The unit commits when the
    /// operation answers `Ok` with no tool error in band.
    async fn operation<T, F, S>(
        &self,
        method: &Method,
        addressed: &Addressed,
        declared: Option<&str>,
        unit: bool,
        stopped: S,
        inner: F,
    ) -> Result<T, McpError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, McpError>> + Send,
        S: Future<Output = Result<T, McpError>> + Send,
    {
        let (binding, timeout) = if unit {
            (self.unit_of_work.as_ref(), self.operation_timeout)
        } else {
            (None, None)
        };

        // Its own unit of work: one session request carries many operations.
        let correlation = Correlation::inherited().child();
        // One `mcp.operation.name` for the conventions' three (`mcp.tool.name`,
        // `mcp.prompt.name`, `mcp.resource.uri`); `mcp.method.name` says which.
        let operation = nest_rs_core::operation_span!(
            crate::unit::OPERATION,
            &correlation,
            mcp.method.name = method.sent(),
            mcp.operation.name = addressed.spanned(),
        );
        let named = method.named();
        // Filed outside the scope `scoped` installs, so it holds its own correlation.
        let line = OperationLine::open(named, declared, correlation.clone(), operation.clone());

        let deadline = timeout.map(|timeout| (timeout, tokio::time::Instant::now() + timeout));
        let spec = UnitSpec::new(
            crate::unit::OPERATION,
            ExecutorScope::Request,
            TransactionPolicy::PerUnit,
        );
        let spec = match deadline {
            Some((_, at)) => spec.with_deadline(at),
            None => spec,
        };
        let stopped_first = &AtomicBool::new(false);
        let view = || self.view(method, addressed);
        let scoped: BoxFuture<'_, Answered<T>> = Box::pin(nest_rs_core::with_request_scope(
            nest_rs_core::current_request_scope(),
            correlation,
            run_unit(
                binding,
                spec,
                async {
                    let dispatched =
                        self.dispatched(named, unit, view, deadline.map(|(_, at)| at), inner);
                    let cut = async move {
                        match deadline {
                            Some((timeout, at)) => tokio::time::timeout_at(at, dispatched)
                                .await
                                .unwrap_or_else(|_| {
                                    Answered::refused(timed_out(named, declared, timeout))
                                }),
                            None => dispatched.await,
                        }
                    };
                    tokio::select! {
                        biased;
                        answered = cut => answered,
                        answer = stopped => {
                            stopped_first.store(true, Ordering::Relaxed);
                            Answered { answer, commits: false }
                        }
                    }
                },
                |answered| answered.commits,
                |_| Answered::refused(opaque_answer()),
            ),
        ));

        async move {
            let ended = self
                .detached
                .run(nest_rs_core::panic::contain(scoped))
                .await;
            match ended {
                Some(Ok(answered)) if stopped_first.load(Ordering::Relaxed) => {
                    line.file(operation_log::CANCELLED);
                    answered.answer
                }
                Some(Ok(Answered {
                    answer: Ok(value), ..
                })) => {
                    line.file(operation_log::OK);
                    Ok(value)
                }
                Some(Ok(Answered {
                    answer: Err(error), ..
                })) => {
                    line.file(operation_log::ERROR);
                    Err(error)
                }
                Some(Err(payload)) => {
                    line.unwound(&*payload);
                    Err(opaque_answer())
                }
                None => {
                    line.file(operation_log::CANCELLED);
                    Err(cancelled_before_completion())
                }
            }
        }
        .instrument(operation)
        .await
    }

    /// `inner` through the request's own site, unless `unit` is `false`; the
    /// operation's deadline, `at`, made ambient for the site a `#[tools]`
    /// operation reaches inside it. `method` is the one its line names.
    async fn dispatched<T, F, V>(
        &self,
        method: &'static str,
        unit: bool,
        view: V,
        at: Option<tokio::time::Instant>,
        inner: F,
    ) -> Answered<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, McpError>> + Send,
        V: FnOnce() -> McpView,
    {
        let chains = match (&self.chains, unit) {
            (RequestChains::Composed(chains), true) => chains,
            (RequestChains::Refused, true) => return Answered::refused(opaque_answer()),
            (RequestChains::Bare, _) | (_, false) => return Answered::of(inner.await),
        };
        let inner = match at {
            Some(at) => Either::Left(nest_rs_core::with_ambient(Arc::new(Deadline(at)), inner)),
            None => Either::Right(inner),
        };
        let mut slot = None;
        let outcome = {
            let mut view = view();
            let mut tail = RequestTail {
                inner: Some(inner),
                slot: &mut slot,
            };
            let mut cx = new_unit_context(crate::operation::REQUEST.handler(), &mut view, None, at);
            dispatch(&mut cx, chains, &mut tail).await
        };
        Answered::dispatched(method, outcome, slot)
    }

    /// The view of the request `method` addressing `addressed`, naming the
    /// host the mount reads declares it.
    fn view(&self, method: &Method, addressed: &Addressed) -> McpView {
        let kind = match addressed {
            Addressed::Tool(_) => Some(McpOperationKind::Tool),
            Addressed::Prompt(_) if method.sent() == GetPromptRequestMethod::VALUE => {
                Some(McpOperationKind::Prompt)
            }
            _ => None,
        };
        let name = addressed.spanned();
        let host = match (kind, name, self.index.as_deref()) {
            (Some(kind), Some(name), Some(index)) => index.host(kind, name),
            _ => None,
        };
        McpView::request(method.as_cow(), kind, name.map(str::to_owned), host)
    }
}

/// What one operation answered, and whether its unit of work commits.
struct Answered<T> {
    answer: Result<T, McpError>,
    commits: bool,
}

impl<T: 'static> Answered<T> {
    /// What rmcp answered, the unit committing on an `Ok` that reports no
    /// tool error in band.
    fn of(answer: Result<T, McpError>) -> Self {
        let commits = answer
            .as_ref()
            .is_ok_and(|value| !reports_tool_error(value));
        Self { answer, commits }
    }

    /// `error`, the unit rolled back.
    fn refused(error: McpError) -> Self {
        Self {
            answer: Err(error),
            commits: false,
        }
    }

    /// What the site of a request for `method` came to: what its tail wrote,
    /// or its failure answered by the edge's one rule.
    fn dispatched(
        method: &'static str,
        outcome: Outcome,
        slot: Option<Result<T, McpError>>,
    ) -> Self {
        match (outcome, slot) {
            (Ok(_), Some(answer)) => Self::of(answer),
            (Err(failure), Some(answer)) if failure.is_answered() => Self {
                answer,
                commits: false,
            },
            (Ok(_), None) => {
                tracing::error!(
                    target: crate::TARGET,
                    method,
                    "an MCP operation settled without an answer; it is refused",
                );
                Self::refused(opaque_answer())
            }
            (Err(failure), _) => Self::refused(answer(&failure, None)),
        }
    }
}

/// Whether `value` is a tool call's result that reports the tool's error —
/// a `CallToolResult` with `isError: true`, how the MCP specification reports
/// a tool that failed — which its unit of work does not commit.
fn reports_tool_error(value: &dyn Any) -> bool {
    let result = match value.downcast_ref::<CallToolResponse>() {
        Some(CallToolResponse::Complete(result)) => Some(result),
        Some(_) => None,
        None => value.downcast_ref::<CallToolResult>(),
    };
    result.is_some_and(|result| result.is_error == Some(true))
}

/// The request's own site's tail: rmcp's routing, its answer written into the
/// operation's slot. An error, or a tool error reported in band, fails the
/// unit, answered: the answer is what rmcp wrote.
struct RequestTail<'s, T, F> {
    inner: Option<F>,
    slot: &'s mut Option<Result<T, McpError>>,
}

impl<T, F> Tail for RequestTail<'_, T, F>
where
    T: Send + 'static,
    F: Future<Output = Result<T, McpError>> + Send,
{
    async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
        let Some(inner) = self.inner.take() else {
            unreachable!("a unit runs its tail once");
        };
        let answer = inner.await;
        let failed = match &answer {
            Ok(value) => reports_tool_error(value).then(|| Failure::from(ToolReportedError)),
            Err(error) => Some(Failure::from(error.clone())),
        };
        *self.slot = Some(answer);
        match failed {
            None => Ok(settled()),
            Some(failure) => Err(failure.answered(settled())),
        }
    }
}

/// What an operation past its deadline is answered: an internal error in the
/// class HTTP's request timeout answers (`UNAVAILABLE`), with `timeout` as its
/// reason, said once at `warn` for the operator.
fn timed_out(method: &str, operation: Option<&str>, timeout: Duration) -> McpError {
    tracing::warn!(
        target: crate::TARGET,
        method,
        operation,
        reason = TIMEOUT_REASON,
        operation_timeout_secs = timeout.as_secs(),
        "mcp operation not answered within its deadline; it is cut",
    );
    McpError::internal_error(
        "the operation was not answered in time",
        Some(serde_json::json!({
            "code": nest_rs_core::problem::code::UNAVAILABLE.as_str(),
            "reason": TIMEOUT_REASON,
        })),
    )
}

/// The `reason` a deadline's refusal carries, beside its `code`.
const TIMEOUT_REASON: &str = "timeout";

/// What rmcp is handed for an operation stopped before it settled — never a
/// claim the work was done, nor that the server failed: the `UNAVAILABLE`
/// class an operation past its deadline answers. No client reads it today:
/// rmcp drops the answer to a request its peer cancelled, and the transport
/// ends the stream of an operation it stops.
fn cancelled_before_completion() -> McpError {
    McpError::internal_error(
        "the operation was cancelled before it completed",
        Some(serde_json::json!({
            "code": nest_rs_core::problem::code::UNAVAILABLE.as_str(),
        })),
    )
}

/// The JSON-RPC method an operation answers: one the protocol defines, which
/// rmcp names as a constant, or a custom one only the client named.
enum Method {
    Defined(&'static str),
    Custom(String),
}

impl Method {
    /// As sent: the span's `mcp.method.name`, which the conventions require
    /// whatever it is.
    fn sent(&self) -> &str {
        match self {
            Self::Defined(method) => method,
            Self::Custom(method) => method,
        }
    }

    /// What a line names: the protocol's method, or
    /// [`OTHER`](operation_log::OTHER) for one only the client named.
    fn named(&self) -> &'static str {
        match self {
            Self::Defined(method) => method,
            Self::Custom(_) => operation_log::OTHER,
        }
    }

    /// As sent, for the operation's view.
    fn as_cow(&self) -> Cow<'static, str> {
        match self {
            Self::Defined(method) => Cow::Borrowed(method),
            Self::Custom(method) => Cow::Owned(method.clone()),
        }
    }
}

impl From<&'static str> for Method {
    fn from(method: &'static str) -> Self {
        Self::Defined(method)
    }
}

impl From<String> for Method {
    fn from(method: String) -> Self {
        Self::Custom(method)
    }
}

/// What an operation addressed, as the client named it.
enum Addressed {
    /// Nothing: the method names nothing.
    Nothing,
    /// A tool.
    Tool(String),
    /// A prompt.
    Prompt(String),
    /// A resource, by its URI.
    Resource(String),
    /// A task, by its id, which no convention names: read only to say a
    /// refusal of it by kind, never recorded.
    Task(String),
}

impl Addressed {
    /// What the span records as `mcp.operation.name`: the conventions' tool,
    /// prompt or resource, their stated exception.
    fn spanned(&self) -> Option<&str> {
        match self {
            Self::Nothing | Self::Task(_) => None,
            Self::Tool(name) | Self::Prompt(name) | Self::Resource(name) => Some(name),
        }
    }

    /// What a line names: a tool by the name `host` declares — read off the
    /// mount's `index`, or `host`'s `get_tool` where it cannot say — and
    /// [`OTHER`](operation_log::OTHER) for one it does not; nothing else the
    /// client named, which no declaration answers for.
    fn declared(&self, host: &impl ServerHandler, index: Option<&MountIndex>) -> Option<&str> {
        match self {
            Self::Tool(name) => {
                let declared = index
                    .and_then(|index| index.tools.declares(name))
                    .unwrap_or_else(|| host.get_tool(name).is_some());
                Some(if declared { name } else { operation_log::OTHER })
            }
            Self::Nothing | Self::Prompt(_) | Self::Resource(_) | Self::Task(_) => None,
        }
    }

    /// `error` as the client is answered: rmcp's own refusals of a name no
    /// host serves quote it, so each is said by kind.
    fn said_by_kind(&self, method: &Method, error: McpError) -> McpError {
        match (method, self) {
            (Method::Custom(sent), _) if refuses_method(&error, sent) => method_not_found(),
            (_, Self::Prompt(name)) if refuses_prompt(&error, name) => prompt_not_found(error.data),
            (_, Self::Task(id)) if refuses_task(&error, id) => task_not_found(),
            _ => error,
        }
    }
}

/// One operation's `mcp.operation` line, filed exactly once — by the end the
/// dispatch saw, else by `Drop` as [`CANCELLED`](operation_log::CANCELLED).
/// It names the server's values only: the protocol's method or
/// [`OTHER`](operation_log::OTHER), and a tool by the name a host declares.
struct OperationLine<'a> {
    method: &'static str,
    declared: Option<&'a str>,
    /// Entered to file the line, which sits outside the scope the operation installs.
    correlation: Correlation,
    /// The operation's own span; [`Span::none`](tracing::Span::none) for a
    /// notification, which opens none.
    span: tracing::Span,
    started: std::time::Instant,
    filed: bool,
}

impl<'a> OperationLine<'a> {
    fn open(
        method: &'static str,
        declared: Option<&'a str>,
        correlation: Correlation,
        span: tracing::Span,
    ) -> Self {
        Self {
            method,
            declared,
            correlation,
            span,
            started: std::time::Instant::now(),
            filed: false,
        }
    }

    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
    }

    /// The operation unwound: file `panic`, and give the operator what the
    /// client is not told.
    fn unwound(mut self, payload: &(dyn std::any::Any + Send)) {
        self.emit(operation_log::PANIC);
        RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "mcp operation panicked; its client is answered with an internal error",
                method = self.method,
                operation = self.declared,
            );
        });
    }

    fn emit(&mut self, outcome: &'static str) {
        self.filed = true;
        RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            nest_rs_core::operation_line!(
                crate::unit::OPERATION,
                span: &self.span,
                outcome: outcome,
                started: self.started,
                // Flat, never dotted: a dotted field name is ambiguous to
                // `tracing`'s parser beside a path target.
                method = self.method,
                operation = self.declared,
            );
        });
    }
}

impl Drop for OperationLine<'_> {
    fn drop(&mut self) {
        if !self.filed {
            self.emit(if std::thread::panicking() {
                operation_log::PANIC
            } else {
                operation_log::CANCELLED
            });
        }
    }
}

/// Delegate one request-shaped method inside the full nesting. Every invocation
/// states `$addressed`, [`Addressed::Nothing`] out loud; it is read before the
/// call consumes the parameters.
macro_rules! request_method {
    (
        $name:ident ( $($arg:ident : $arg_ty:ty),* ) -> $out:ty,
        $method:expr,
        $addressed:expr $(,)?
    ) => {
        fn $name(
            &self,
            $($arg: $arg_ty,)*
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<$out, McpError>> + MaybeSendFuture + '_ {
            async move {
                let ambient = McpAmbient::from_extensions(&context.extensions);
                let method = Method::from($method);
                let addressed: Addressed = $addressed;
                let cancelled = context.ct.clone().cancelled_owned();
                let inner: BoxFuture<'_, Result<$out, McpError>> =
                    Box::pin(self.inner.$name($($arg,)* context));
                self.dispatch(
                    &method,
                    &addressed,
                    ambient,
                    true,
                    async move {
                        cancelled.await;
                        Err(cancelled_before_completion())
                    },
                    inner,
                )
                .await
                .map_err(|error| addressed.said_by_kind(&method, error))
            }
        }
    };
}

/// Delegate one notification: the request's context is installed, and
/// nothing else is.
macro_rules! notification_method {
    ($name:ident ( $($arg:ident : $arg_ty:ty),* ), $method:expr $(,)?) => {
        fn $name(
            &self,
            $($arg: $arg_ty,)*
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            let ambient = McpAmbient::from_extensions(&context.extensions);
            let method = Method::from($method);
            let handled: BoxFuture<'_, _> =
                Box::pin(nest_rs_core::panic::contain(self.inner.$name($($arg,)* context)));
            let run = async move {
                let correlation = Correlation::inherited();
                let line = OperationLine::open(
                    method.named(),
                    None,
                    correlation.clone(),
                    tracing::Span::none(),
                );
                nest_rs_core::with_request_scope(
                    nest_rs_core::current_request_scope(),
                    correlation,
                    async move {
                        match self.units.detached.run(handled).await {
                            Some(Ok(())) => line.file(operation_log::OK),
                            Some(Err(payload)) => line.unwound(&*payload),
                            None => line.file(operation_log::CANCELLED),
                        }
                    },
                )
                .await
            };
            match ambient {
                Some(ambient) => Either::Left(ambient.carried.carry(run)),
                None => Either::Right(run),
            }
        }
    };
}

#[deny(clippy::missing_trait_methods)]
impl<H: ServerHandler> ServerHandler for PropagatingHandler<H> {
    request_method!(ping() -> (), PingRequestMethod::VALUE, Addressed::Nothing);
    request_method!(
        initialize(request: InitializeRequestParams) -> InitializeResult,
        InitializeResultMethod::VALUE,
        Addressed::Nothing,
    );
    request_method!(
        discover() -> DiscoverResult,
        DiscoverRequestMethod::VALUE,
        Addressed::Nothing,
    );

    request_method!(
        call_tool(request: CallToolRequestParams) -> CallToolResponse,
        CallToolRequestMethod::VALUE,
        Addressed::Tool(request.name.to_string()),
    );
    request_method!(
        list_tools(request: Option<PaginatedRequestParams>) -> ListToolsResult,
        ListToolsRequestMethod::VALUE,
        Addressed::Nothing,
    );

    request_method!(
        get_prompt(request: GetPromptRequestParams) -> GetPromptResponse,
        GetPromptRequestMethod::VALUE,
        Addressed::Prompt(request.name.clone()),
    );
    request_method!(
        list_prompts(request: Option<PaginatedRequestParams>) -> ListPromptsResult,
        ListPromptsRequestMethod::VALUE,
        Addressed::Nothing,
    );

    request_method!(
        read_resource(request: ReadResourceRequestParams) -> ReadResourceResponse,
        ReadResourceRequestMethod::VALUE,
        Addressed::Resource(request.uri.clone()),
    );
    request_method!(
        list_resources(request: Option<PaginatedRequestParams>) -> ListResourcesResult,
        ListResourcesRequestMethod::VALUE,
        Addressed::Nothing,
    );
    request_method!(
        list_resource_templates(request: Option<PaginatedRequestParams>)
            -> ListResourceTemplatesResult,
        ListResourceTemplatesRequestMethod::VALUE,
        Addressed::Nothing,
    );
    request_method!(
        subscribe(request: SubscribeRequestParams) -> (),
        SubscribeRequestMethod::VALUE,
        Addressed::Resource(request.uri.clone()),
    );
    request_method!(
        unsubscribe(request: UnsubscribeRequestParams) -> (),
        UnsubscribeRequestMethod::VALUE,
        Addressed::Resource(request.uri.clone()),
    );

    request_method!(
        complete(request: CompleteRequestParams) -> CompleteResult,
        CompleteRequestMethod::VALUE,
        // `Reference` is `#[non_exhaustive]`: a variant rmcp adds files no name.
        match &request.r#ref {
            Reference::Prompt(prompt) => Addressed::Prompt(prompt.name.clone()),
            Reference::Resource(resource) => Addressed::Resource(resource.uri.clone()),
            _ => Addressed::Nothing,
        },
    );
    request_method!(
        set_level(request: SetLevelRequestParams) -> (),
        SetLevelRequestMethod::VALUE,
        Addressed::Nothing,
    );

    request_method!(
        get_task(request: GetTaskParams) -> GetTaskResult,
        GetTaskMethod::VALUE,
        Addressed::Task(request.task_id.clone()),
    );
    request_method!(
        update_task(request: UpdateTaskParams) -> (),
        UpdateTaskMethod::VALUE,
        Addressed::Task(request.task_id.clone()),
    );
    request_method!(
        cancel_task(request: CancelTaskParams) -> (),
        CancelTaskMethod::VALUE,
        Addressed::Task(request.task_id.clone()),
    );

    request_method!(
        on_custom_request(request: CustomRequest) -> CustomResult,
        request.method.clone(),
        Addressed::Nothing,
    );

    notification_method!(
        on_cancelled(notification: CancelledNotificationParam),
        CancelledNotificationMethod::VALUE,
    );
    notification_method!(
        on_progress(notification: ProgressNotificationParam),
        ProgressNotificationMethod::VALUE,
    );
    notification_method!(on_initialized(), InitializedNotificationMethod::VALUE);
    notification_method!(
        on_roots_list_changed(),
        RootsListChangedNotificationMethod::VALUE,
    );
    notification_method!(
        on_custom_notification(notification: CustomNotification),
        notification.method.clone(),
    );

    /// No context and no unit of work: a transaction would pin a pooled
    /// connection for the subscription's life. At the shutdown signal it answers
    /// `Ok(())`, which rmcp sends as the schema's final result for a graceful
    /// teardown, and files `cancelled`.
    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let ambient = McpAmbient::from_extensions(&context.request_context().extensions);
        let going_away = self.units.detached.going_away();
        let inner: BoxFuture<'_, Result<(), McpError>> = Box::pin(self.inner.listen(context));
        self.dispatch(
            &Method::from(SubscriptionsListenRequestMethod::VALUE),
            &Addressed::Nothing,
            ambient,
            false,
            async move {
                going_away.await;
                Ok(())
            },
            inner,
        )
        .await
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.inner.accepted_subscription_filter(requested)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.inner.get_tool(name)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        self.inner.supported_protocol_versions()
    }

    fn get_info(&self) -> ServerConfig {
        self.inner.get_info()
    }

    /// Delegated, or a host narrowing its versions would have its answer dropped.
    fn negotiate_initialize(
        &self,
        request: &InitializeRequestParams,
    ) -> Result<InitializeResult, McpError> {
        self.inner.negotiate_initialize(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not `LATEST`, the default, so a substituted request shows.
    const THE_CALLER_ASKS_FOR: ProtocolVersion = ProtocolVersion::V_2025_06_18;

    /// Neither the version asked for nor any default, so an overwritten verdict shows.
    const THE_HOST_ANSWERS_WITH: ProtocolVersion = ProtocolVersion::V_2024_11_05;

    #[derive(Clone)]
    struct NegotiatingHost;

    impl ServerHandler for NegotiatingHost {
        fn negotiate_initialize(
            &self,
            request: &InitializeRequestParams,
        ) -> Result<InitializeResult, McpError> {
            assert_eq!(
                request.protocol_version, THE_CALLER_ASKS_FOR,
                "the wrapper substituted a request of its own",
            );
            let mut negotiated = ServerConfig::default();
            negotiated.protocol_version = THE_HOST_ANSWERS_WITH;
            Ok(negotiated)
        }
    }

    /// An operation stopped before it settled is no failure of the server's:
    /// its answer says so in its own sentence, under the `503` class, as an
    /// operation past its deadline does.
    #[test]
    fn an_operation_stopped_before_it_settled_names_the_unavailable_class() {
        let stopped = cancelled_before_completion();

        assert_eq!(stopped.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert_eq!(
            stopped.message,
            "the operation was cancelled before it completed"
        );
        assert_eq!(
            stopped.data,
            Some(serde_json::json!({
                "code": nest_rs_core::problem::code::UNAVAILABLE.as_str(),
            })),
        );
    }

    /// A direct call: rmcp reaches this through `initialize`, so no wire probe
    /// tells a delegated call from an inherited one.
    #[test]
    fn the_wrapper_asks_the_inner_host_to_negotiate() {
        let wrapper = PropagatingHandler::new(
            NegotiatingHost,
            OperationUnits::new(None, DetachedWork::new(), None, None, RequestChains::Bare),
        );
        let mut request = InitializeRequestParams::default();
        request.protocol_version = THE_CALLER_ASKS_FOR;

        let negotiated = wrapper
            .negotiate_initialize(&request)
            .expect("the caller's own request reaches the host");

        assert_eq!(
            negotiated.protocol_version, THE_HOST_ANSWERS_WITH,
            "the host's negotiation was rebuilt rather than delegated",
        );
    }
}

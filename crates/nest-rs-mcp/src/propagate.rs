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
//! | Kind | Request context | Unit stage | Unit of work | Deadline | Span and line |
//! |---|---|---|---|---|---|
//! | Requests (`tools/call`, `prompts/get`, `resources/read`, `completion/complete`, `logging/setLevel`, `tasks/*`, lifecycle, custom) | yes | yes | yes | yes | yes |
//! | `subscriptions/listen` | yes | no | no — it outlives any sane transaction | no | yes |
//! | Notifications | yes | no | no — nothing to commit or roll back on | no | yes |
//! | Synchronous accessors (`get_info`, `get_tool`, …) | — | — | — | — | no — no work is dispatched |
//!
//! The request context is the `/mcp` request's: its span, scope and
//! correlation, and what its guards and layers made ambient.
//!
//! An operation files `ok`, or `error` for an answer its unit would not
//! commit — a tool's error in band among them, which its span records as
//! `tool_error`, and a JSON-RPC error, whose code its span records as
//! OpenTelemetry's MCP conventions ask ([`FailedAs`]) — and for one outliving
//! `McpConfig::operation_timeout`, which every request but
//! `subscriptions/listen` runs under; `panic` when it unwinds (contained, the
//! client answered with an internal error rather than left to its timeout); or
//! `cancelled` when its client cancels it or the transport stops it. The line
//! is filed by [`OperationLine`], so an end nobody saw still files one.

#![expect(
    deprecated,
    reason = "rmcp still routes the deprecated methods for legacy protocol versions"
)]

use std::any::Any;
use std::borrow::Cow;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::Either;
use nest_rs_core::__private::{Tail, UnitChains, dispatch, new_unit_context, settled};
use nest_rs_core::operation_log::{FiledOnce, Line, Unwinding};
use nest_rs_core::{
    AmbientValue, Correlation, Deadline, Failure, Outcome, RequestContinuation, ServedAddress,
    UnitContext, operation_log,
};
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_http::DetachedWork;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResponse, CallToolResult,
    CancelTaskMethod, CancelTaskParams, CancelledNotificationMethod, CancelledNotificationParam,
    CompleteRequestMethod, CompleteRequestParams, CompleteResult, ConstString, CustomNotification,
    CustomRequest, CustomResult, DiscoverRequestMethod, DiscoverResult, ErrorCode,
    GetPromptRequestMethod, GetPromptRequestParams, GetPromptResponse, GetSkillRequestMethod,
    GetSkillRequestParams, GetSkillResult, GetTaskMethod, GetTaskParams, GetTaskResult,
    InitializeRequestParams, InitializeResult, InitializeResultMethod,
    InitializedNotificationMethod, ListPromptsRequestMethod, ListPromptsResult,
    ListResourceTemplatesRequestMethod, ListResourceTemplatesResult, ListResourcesRequestMethod,
    ListResourcesResult, ListSkillsRequestMethod, ListSkillsResult, ListToolsRequestMethod,
    ListToolsResult, PaginatedRequestParams, PingRequestMethod, ProgressNotificationMethod,
    ProgressNotificationParam, ProtocolVersion, ReadResourceDirectoryRequestMethod,
    ReadResourceDirectoryRequestParams, ReadResourceDirectoryResult, ReadResourceRequestMethod,
    ReadResourceRequestParams, ReadResourceResponse, Reference, RequestId,
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
    AVAILABLE_PROMPTS, ClassData, ToolReportedError, answer, classed_method_not_found,
    method_not_found, opaque_answer, prompt_not_found, refuses_method, refuses_prompt,
    refuses_task, refuses_tool, task_not_found, tool_not_found, withholding,
};
use crate::guard::BoxFuture;
use crate::operation::McpOperationKind;
use crate::registry::MountIndex;
use crate::unit::{
    EXECUTE_TOOL, GEN_AI_OPERATION_NAME, GEN_AI_PROMPT_NAME, GEN_AI_TOOL_NAME, HTTP,
    JSONRPC_REQUEST_ID, MCP_METHOD_NAME, MCP_RESOURCE_URI, NETWORK_PROTOCOL_NAME,
    NETWORK_TRANSPORT, RPC_RESPONSE_STATUS_CODE, TCP,
};
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
/// Built by the endpoint an MCP path mounts, one per session, never by hand.
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
        requested: &Requested,
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
        let declared = requested
            .addressed
            .declared(&self.inner, self.units.index.as_deref());
        self.units
            .dispatch(requested, declared, ambient, unit, stopped, inner)
            .await
    }
}

/// What one mount runs each request as: an operation unit — its own site, a
/// unit of work, a deadline and a stop. Every session's handler shares it.
#[derive(Clone)]
pub(crate) struct OperationUnits {
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    detached: DetachedWork,
    /// What one request operation may run for; `None` ⇒ no deadline.
    operation_timeout: Option<Duration>,
    /// What the mount's hosts declare; `None` asks the host's `get_tool`.
    index: Option<Arc<MountIndex>>,
    chains: RequestChains,
    /// The path the mount is served at, made ambient for every operation it
    /// runs; `None` on a hand-built endpoint.
    served_at: Option<Arc<ServedPath>>,
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
            served_at: None,
        }
    }

    /// The same units, naming the mount at `served_at` on their lines.
    pub(crate) fn served_at(self, served_at: Option<Arc<ServedPath>>) -> Self {
        Self { served_at, ..self }
    }

    /// Run `inner` as one operation, under the `/mcp` request's context
    /// `ambient` carries.
    ///
    /// `requested` is the JSON-RPC request as the client sent it, which the
    /// span and the view record as the conventions ask; the line, the span's
    /// tool and prompt and its name read `declared` alone, what the mount
    /// declares under that name. `unit` is
    /// `false` for an operation that runs neither the request's site, a unit
    /// of work nor the deadline, which cuts the body, never its settle, so a
    /// commit is never cut. `stopped` resolves when the operation is to end
    /// before it answers, with what the client is answered; like the deadline
    /// it cuts the body alone, so a stop arriving while the unit settles leaves
    /// the operation to end as it answered.
    async fn dispatch<T, F, S>(
        &self,
        requested: &Requested,
        declared: ServedAddress<'_>,
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
            Box::pin(self.operation(requested, declared, unit, stopped, inner));
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
        requested: &Requested,
        declared: ServedAddress<'_>,
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

        let Requested {
            method, addressed, ..
        } = requested;
        // Its own unit of work: one session request carries many operations.
        let correlation = Correlation::inherited().child();
        let operation = requested.span(declared, &correlation);
        let named = method.named();
        // Only the `#[resource]` serving a URI knows the template it matched,
        // and the line is filed outside the unit it runs in.
        let served_resource = matches!(addressed, Addressed::Resource(_))
            .then(|| Arc::new(ServedResource::default()));
        let inner = match &served_resource {
            Some(served) => Either::Left(nest_rs_core::with_ambient(Arc::clone(served), inner)),
            None => Either::Right(inner),
        };
        // The site a `#[tools]` operation reaches inside names the mount too.
        let inner = match &self.served_at {
            Some(path) => Either::Left(nest_rs_core::with_ambient(Arc::clone(path), inner)),
            None => Either::Right(inner),
        };
        let address = &LineAddress {
            declared,
            served_resource,
            path: self.served_at.as_deref().map(|served| &*served.0),
        };
        // Filed outside the scope `scoped` installs, so it holds its own correlation.
        let line = OperationLine::open(named, address, correlation.clone(), operation.clone());

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
        let view = || self.view(requested);
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
                                    Answered::refused(timed_out(named, address, timeout))
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
            // Filed whatever the answer: a tool error reported in band, and a
            // failure the tool handled, kept a chain all the same.
            let (ended, withheld) =
                withholding(self.detached.run(nest_rs_core::panic::contain(scoped))).await;
            for withheld in withheld {
                crate::error::file(withheld, address.named());
            }
            match ended {
                Some(Ok(answered)) if stopped_first.load(Ordering::Relaxed) => {
                    line.file(operation_log::CANCELLED);
                    answered.answer
                }
                Some(Ok(answered)) => {
                    match answered.failed_as() {
                        None => line.file(operation_log::OK),
                        Some(failed_as) => OperationLine::failed(line, failed_as),
                    }
                    answered.answer
                }
                Some(Err(payload)) => {
                    let answer = opaque_answer();
                    OperationLine::unwound(line, &*payload, Some(answer.code));
                    Err(answer)
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
    fn view(
        &self,
        Requested {
            method, addressed, ..
        }: &Requested,
    ) -> McpView {
        let kind = match addressed {
            Addressed::Tool(_) => Some(McpOperationKind::Tool),
            Addressed::Prompt(_) if method.sent() == GetPromptRequestMethod::VALUE => {
                Some(McpOperationKind::Prompt)
            }
            Addressed::Resource(_) if method.sent() == ReadResourceRequestMethod::VALUE => {
                Some(McpOperationKind::Resource)
            }
            _ => None,
        };
        let name = addressed.as_sent();
        let host = match (kind, name, self.index.as_deref()) {
            (Some(kind), Some(name), Some(index)) => index.host(kind, name),
            _ => None,
        };
        McpView::request(method.as_cow(), kind, name.map(str::to_owned), host)
            .served_at(self.served_at.as_ref().map(|path| Arc::clone(&path.0)))
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

    /// How its unit failed, read off what decided the commit, so the line and
    /// the span never say `ok` for a unit that rolled back: `None` when it
    /// commits.
    fn failed_as(&self) -> Option<FailedAs> {
        if self.commits {
            return None;
        }
        Some(match &self.answer {
            Err(error) => FailedAs::Answered(error.code),
            Ok(value) if reports_tool_error(value) => FailedAs::ToolError,
            Ok(_) => FailedAs::Unit,
        })
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
                    mcp_method = method,
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

/// How an operation whose unit rolled back failed, which its span records as
/// OpenTelemetry's MCP semantic conventions ask; its line files `error`.
enum FailedAs {
    /// A tool call answered with its error in band, `isError: true`: the
    /// conventions' `tool_error`.
    ToolError,
    /// Answered with a JSON-RPC error: its code, as `rpc.response.status_code`,
    /// and as `error.type` unless the conventions read it as the caller's own
    /// ([`is_the_callers`]).
    Answered(ErrorCode),
    /// Answered with a value, the unit failed around it by a layer: the
    /// outcome word, `error`.
    Unit,
}

/// The `error.type` OpenTelemetry's MCP semantic conventions give a
/// `CallToolResult` answered with `isError: true`.
const TOOL_ERROR: &str = "tool_error";

/// Whether OpenTelemetry's MCP conventions read `code`, answered by a server,
/// as a request the caller sent that the server could not serve — a parse
/// error, an invalid request, a method, tool, prompt or resource no host
/// serves, arguments that do not decode — rather than the server's failure.
/// Such an operation fails nothing on the server's span, as an HTTP `4xx`
/// does not: it records its code alone.
fn is_the_callers(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::PARSE_ERROR
            | ErrorCode::INVALID_REQUEST
            | ErrorCode::METHOD_NOT_FOUND
            | ErrorCode::INVALID_PARAMS
            | ErrorCode::RESOURCE_NOT_FOUND
    )
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
fn timed_out(mcp_method: &str, address: &LineAddress<'_>, timeout: Duration) -> McpError {
    let named = address.named();
    tracing::warn!(
        target: crate::TARGET,
        mcp_method,
        path = named.path,
        tool = named.tool,
        prompt = named.prompt,
        resource = named.resource,
        reason = TIMEOUT_REASON,
        timeout_ms = timeout.as_millis(),
        "mcp operation not answered within its deadline; it is cut",
    );
    McpError::internal_error(
        "the operation was not answered in time",
        ClassData {
            reason: Some(TIMEOUT_REASON),
            ..ClassData::of(nest_rs_core::code::UNAVAILABLE)
        }
        .into_data(),
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
        ClassData::of(nest_rs_core::code::UNAVAILABLE).into_data(),
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

/// One JSON-RPC request or notification an operation answers, as the client
/// sent it.
struct Requested {
    method: Method,
    /// `None` for a notification, which carries none.
    id: Option<RequestId>,
    addressed: Addressed,
}

impl Requested {
    /// A notification of `method`, which names nothing.
    fn notification(method: Method) -> Self {
        Self {
            method,
            id: None,
            addressed: Addressed::Nothing,
        }
    }

    /// The operation's span, under `correlation`, as the MCP conventions
    /// describe a request or notification a server processes.
    fn span(&self, declared: ServedAddress<'_>, correlation: &Correlation) -> tracing::Span {
        // The name, the tool and the prompt are what a host declares, `_OTHER`
        // otherwise, so neither a span's name nor the duration keyed by them
        // holds a word only the client wrote.
        nest_rs_core::operation_span!(
            crate::unit::OPERATION,
            correlation,
            otel.name = %SpanName::of(&self.method, declared),
            { MCP_METHOD_NAME } = self.method.sent(),
            { JSONRPC_REQUEST_ID } = self.id.as_ref().map(tracing::field::display),
            { GEN_AI_TOOL_NAME } = declared.tool,
            { GEN_AI_PROMPT_NAME } = declared.prompt,
            { GEN_AI_OPERATION_NAME } = self.addressed.operation_name(),
            { MCP_RESOURCE_URI } = self.addressed.resource_uri(),
            { NETWORK_TRANSPORT } = TCP,
            { NETWORK_PROTOCOL_NAME } = HTTP,
            { RPC_RESPONSE_STATUS_CODE } = tracing::field::Empty,
        )
    }
}

/// An operation's exported name, as the MCP conventions form it: the method,
/// then the tool or prompt a host declares, the method alone where none does —
/// the server's values alone, `_OTHER` for a custom method.
struct SpanName<'a> {
    method: &'static str,
    target: Option<&'a str>,
}

impl<'a> SpanName<'a> {
    fn of(method: &Method, declared: ServedAddress<'a>) -> Self {
        Self {
            method: method.named(),
            target: declared
                .tool
                .or(declared.prompt)
                .filter(|target| *target != operation_log::OTHER),
        }
    }
}

impl std::fmt::Display for SpanName<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.target {
            Some(target) => write!(f, "{} {target}", self.method),
            None => f.write_str(self.method),
        }
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
    /// The tool, prompt or resource the client named, as it named it.
    fn as_sent(&self) -> Option<&str> {
        match self {
            Self::Nothing | Self::Task(_) => None,
            Self::Tool(name) | Self::Prompt(name) | Self::Resource(name) => Some(name),
        }
    }

    /// The span's `gen_ai.operation.name`: [`EXECUTE_TOOL`] on a tool call.
    fn operation_name(&self) -> Option<&'static str> {
        matches!(self, Self::Tool(_)).then_some(EXECUTE_TOOL)
    }

    /// The span's `mcp.resource.uri`: the URI as the client sent it, the
    /// conventions' stated exception.
    fn resource_uri(&self) -> Option<&str> {
        match self {
            Self::Resource(uri) => Some(uri),
            _ => None,
        }
    }

    /// What a line names, as the operation's `mounted` line does: a tool by the
    /// name `host` declares — read off the mount's `index`, or `host`'s
    /// `get_tool` where it cannot say — a prompt by the name the mount's `index`
    /// reads declared, and [`OTHER`](operation_log::OTHER) for one neither
    /// declares; nothing else the client named, a resource's template being
    /// what its unit says ([`LineAddress`]).
    fn declared(&self, host: &impl ServerHandler, index: Option<&MountIndex>) -> ServedAddress<'_> {
        match self {
            Self::Tool(name) => {
                let declared = index
                    .and_then(|index| index.tools.declares(name))
                    .unwrap_or_else(|| host.get_tool(name).is_some());
                ServedAddress::tool(if declared { name } else { operation_log::OTHER })
            }
            Self::Prompt(name) => {
                let declared = index.and_then(|index| index.prompts.declares(name));
                ServedAddress::prompt(if declared == Some(true) {
                    name
                } else {
                    operation_log::OTHER
                })
            }
            Self::Nothing | Self::Resource(_) | Self::Task(_) => ServedAddress::default(),
        }
    }

    /// `error` as the client is answered: rmcp's own refusals of a name no
    /// host serves quote it, so each is said by kind, and its refusals that
    /// quote nothing — a tool no router declares, a protocol method no host
    /// serves — take the class every other refusal carries. Every handler's
    /// answer passes here, at every protocol version.
    fn said_by_kind(&self, method: &Method, error: McpError) -> McpError {
        match (method, self) {
            (Method::Custom(sent), _) if refuses_method(&error, sent) => method_not_found(),
            (_, Self::Tool(_)) if error.data.is_none() && refuses_tool(&error) => tool_not_found(),
            (_, Self::Prompt(name)) if refuses_prompt(&error, name) => {
                prompt_not_found(error.data.and_then(|mut data| {
                    data.get_mut(AVAILABLE_PROMPTS).map(serde_json::Value::take)
                }))
            }
            (_, Self::Task(id)) if refuses_task(&error, id) => task_not_found(),
            _ => classed_method_not_found(error),
        }
    }
}

/// The template a `resources/read`'s `#[resource]` served it through: the
/// server's name for a URI the client chose. The last one running says it, as
/// a host answering not-found hands the URI to the next.
#[derive(Default)]
pub(crate) struct ServedResource(Mutex<Option<&'static str>>);

impl ServedResource {
    /// The unit serving the read matched `template`.
    pub(crate) fn serves(&self, template: &'static str) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(template);
    }

    /// The template that served the read, if a declared one did.
    fn template(&self) -> Option<&'static str> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl AmbientValue for ServedResource {}

/// The path an operation's mount is served at, as its `mounted` lines name
/// it: ambient for the operation, so the site a `#[tools]` operation reaches
/// inside names it too.
pub(crate) struct ServedPath(pub(crate) Arc<str>);

impl AmbientValue for ServedPath {}

/// What an operation's lines name it by.
#[derive(Default)]
struct LineAddress<'a> {
    declared: ServedAddress<'a>,
    /// On an operation a URI addresses, where its unit says the template.
    served_resource: Option<Arc<ServedResource>>,
    /// The path its mount is served at; `None` on a hand-built endpoint.
    path: Option<&'a str>,
}

impl LineAddress<'_> {
    /// `tool`, `prompt` or `resource` as the mount declares it, or
    /// [`OTHER`](operation_log::OTHER) for what it does not, at the mount's
    /// `path`.
    fn named(&self) -> ServedAddress<'_> {
        let named = match &self.served_resource {
            Some(served) => {
                ServedAddress::resource(served.template().unwrap_or(operation_log::OTHER))
            }
            None => self.declared,
        };
        self.path.map_or(named, |path| named.at(path))
    }
}

/// One operation's `mcp.operation` line: by the end the dispatch saw, else
/// `cancelled` when it is dropped first. It names the server's values only:
/// the protocol's method or [`OTHER`](operation_log::OTHER) as `mcp_method`,
/// and what the method addressed as its `mounted` line does.
struct OperationLine<'a> {
    mcp_method: &'static str,
    address: &'a LineAddress<'a>,
    /// Entered to file the line, which sits outside the scope the operation installs.
    correlation: Correlation,
    /// The operation's own span.
    span: tracing::Span,
    started: std::time::Instant,
}

impl<'a> OperationLine<'a> {
    /// Nothing but the operation shares the future its line drops with: an
    /// unwind dropping it is the operation's own.
    fn open(
        mcp_method: &'static str,
        address: &'a LineAddress<'a>,
        correlation: Correlation,
        span: tracing::Span,
    ) -> FiledOnce<Self> {
        FiledOnce::new(
            Self {
                mcp_method,
                address,
                correlation,
                span,
                started: std::time::Instant::now(),
            },
            Unwinding::Own,
        )
    }

    /// The operation failed as `failed_as` says, which its span records in
    /// place of the outcome word its line files, `error`: `tracing-opentelemetry`
    /// keeps every value a field is recorded with, so the class is recorded once
    /// and the line files with the span taken out.
    fn failed(mut line: FiledOnce<Self>, failed_as: FailedAs) {
        let span = std::mem::replace(&mut line.span, tracing::Span::none());
        match failed_as {
            FailedAs::ToolError => operation_log::record_error(&span, TOOL_ERROR),
            FailedAs::Answered(code) => {
                span.record(RPC_RESPONSE_STATUS_CODE, tracing::field::display(code.0));
                if !is_the_callers(code) {
                    operation_log::record_error(&span, &code.0.to_string());
                }
            }
            FailedAs::Unit => operation_log::record_error(&span, operation_log::ERROR),
        }
        operation_log::record_end(&span);
        line.file(operation_log::ERROR);
    }

    /// The operation unwound: file `panic`, the word every edge's span gives an
    /// unwind, beside `answered`, the code its client is answered with — a
    /// notification is answered nothing — and give the operator what the
    /// client is not told.
    fn unwound(
        line: FiledOnce<Self>,
        payload: &(dyn std::any::Any + Send),
        answered: Option<ErrorCode>,
    ) {
        if let Some(code) = answered {
            line.span
                .record(RPC_RESPONSE_STATUS_CODE, tracing::field::display(code.0));
        }
        let (mcp_method, address) = (line.mcp_method, line.address);
        let correlation = line.correlation.clone();
        line.file(operation_log::PANIC);
        RequestContinuation::new(None, correlation).enter(|| {
            let named = address.named();
            nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "mcp operation panicked; its client is answered with an internal error",
                mcp_method,
                path = named.path,
                tool = named.tool,
                prompt = named.prompt,
                resource = named.resource,
            );
        });
    }
}

impl Line for OperationLine<'_> {
    fn file(&mut self, outcome: &'static str) {
        RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            let named = self.address.named();
            nest_rs_core::operation_line!(
                crate::unit::OPERATION,
                span: &self.span,
                outcome: outcome,
                started: self.started,
                // Flat, never dotted: a dotted field name is ambiguous to
                // `tracing`'s parser beside a path target.
                mcp_method = self.mcp_method,
                path = named.path,
                tool = named.tool,
                prompt = named.prompt,
                resource = named.resource,
            );
        });
    }
}

/// Delegate one request-shaped method inside the full nesting. Every invocation
/// states `$addressed`, [`Addressed::Nothing`] out loud; it is read before the
/// call consumes the parameters.
macro_rules! request_method {
    (@delegate $name:ident ( $($arg:ident : $arg_ty:ty),* ) -> $out:ty, $method:expr, $addressed:expr) => {
        fn $name(
            &self,
            $($arg: $arg_ty,)*
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<$out, McpError>> + MaybeSendFuture + '_ {
            async move {
                let ambient = McpAmbient::from_extensions(&context.extensions);
                let requested = Requested {
                    method: Method::from($method),
                    id: Some(context.id.clone()),
                    addressed: $addressed,
                };
                let cancelled = context.ct.clone().cancelled_owned();
                let inner: BoxFuture<'_, Result<$out, McpError>> =
                    Box::pin(self.inner.$name($($arg,)* context));
                self.dispatch(
                    &requested,
                    ambient,
                    true,
                    async move {
                        cancelled.await;
                        Err(cancelled_before_completion())
                    },
                    inner,
                )
                .await
                .map_err(|error| requested.addressed.said_by_kind(&requested.method, error))
            }
        }
    };
    // A method the protocol defines keys its duration by name, so it is one the
    // unit's known set lists: held where the build instantiates the handler.
    (
        $name:ident ( $($arg:ident : $arg_ty:ty),* $(,)? ) -> $out:ty,
        $method:path,
        $addressed:expr $(,)?
    ) => {
        request_method!(@delegate $name ( $($arg : $arg_ty),* ) -> $out, {
            const {
                ::core::assert!(
                    crate::unit::is_protocol_method($method),
                    "a method the protocol defines is listed in `unit::PROTOCOL_METHODS`",
                )
            };
            $method
        }, $addressed);
    };
    (
        $name:ident ( $($arg:ident : $arg_ty:ty),* $(,)? ) -> $out:ty,
        $method:expr,
        $addressed:expr $(,)?
    ) => {
        request_method!(@delegate $name ( $($arg : $arg_ty),* ) -> $out, $method, $addressed);
    };
}

/// Delegate one notification: the request's context is installed, and its
/// span and line opened, and nothing else is.
macro_rules! notification_method {
    (@delegate $name:ident ( $($arg:ident : $arg_ty:ty),* ), $method:expr) => {
        fn $name(
            &self,
            $($arg: $arg_ty,)*
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            let ambient = McpAmbient::from_extensions(&context.extensions);
            let requested = Requested::notification(Method::from($method));
            let handled: BoxFuture<'_, _> =
                Box::pin(nest_rs_core::panic::contain(self.inner.$name($($arg,)* context)));
            let run = async move {
                let correlation = Correlation::inherited().child();
                let operation = requested.span(ServedAddress::default(), &correlation);
                let address = LineAddress::default();
                let line = OperationLine::open(
                    requested.method.named(),
                    &address,
                    correlation.clone(),
                    operation.clone(),
                );
                nest_rs_core::with_request_scope(
                    nest_rs_core::current_request_scope(),
                    correlation,
                    async move {
                        match self.units.detached.run(handled).await {
                            Some(Ok(())) => line.file(operation_log::OK),
                            Some(Err(payload)) => OperationLine::unwound(line, &*payload, None),
                            None => line.file(operation_log::CANCELLED),
                        }
                    },
                )
                .instrument(operation)
                .await
            };
            match ambient {
                Some(ambient) => Either::Left(ambient.carried.carry(run)),
                None => Either::Right(run),
            }
        }
    };
    // As a request's: a notification the protocol defines keys its duration by
    // its method.
    ($name:ident ( $($arg:ident : $arg_ty:ty),* $(,)? ), $method:path $(,)?) => {
        notification_method!(@delegate $name ( $($arg : $arg_ty),* ), {
            const {
                ::core::assert!(
                    crate::unit::is_protocol_method($method),
                    "a method the protocol defines is listed in `unit::PROTOCOL_METHODS`",
                )
            };
            $method
        });
    };
    ($name:ident ( $($arg:ident : $arg_ty:ty),* $(,)? ), $method:expr $(,)?) => {
        notification_method!(@delegate $name ( $($arg : $arg_ty),* ), $method);
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
        list_resource_templates(
            request: Option<PaginatedRequestParams>,
        ) -> ListResourceTemplatesResult,
        ListResourceTemplatesRequestMethod::VALUE,
        Addressed::Nothing,
    );
    request_method!(
        read_resource_directory(
            request: ReadResourceDirectoryRequestParams,
        ) -> ReadResourceDirectoryResult,
        ReadResourceDirectoryRequestMethod::VALUE,
        Addressed::Resource(request.uri.clone()),
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
        get_skill(request: GetSkillRequestParams) -> GetSkillResult,
        GetSkillRequestMethod::VALUE,
        Addressed::Resource(request.uri.clone()),
    );
    request_method!(
        list_skills(request: Option<PaginatedRequestParams>) -> ListSkillsResult,
        ListSkillsRequestMethod::VALUE,
        Addressed::Nothing,
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
        let requested = Requested {
            method: Method::from(SubscriptionsListenRequestMethod::VALUE),
            id: Some(context.request_context().id.clone()),
            addressed: Addressed::Nothing,
        };
        let going_away = self.units.detached.going_away();
        let inner: BoxFuture<'_, Result<(), McpError>> = Box::pin(self.inner.listen(context));
        self.dispatch(
            &requested,
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
                "code": nest_rs_core::code::UNAVAILABLE.as_str(),
            })),
        );
    }

    /// The five codes OpenTelemetry's MCP conventions read as the caller's
    /// fail nothing on the server's span; every other code is the server's
    /// failure, as the conventions say of "any other error code".
    #[test]
    fn only_the_codes_the_conventions_name_are_the_callers() {
        for callers in [
            ErrorCode::PARSE_ERROR,
            ErrorCode::INVALID_REQUEST,
            ErrorCode::METHOD_NOT_FOUND,
            ErrorCode::INVALID_PARAMS,
            ErrorCode::RESOURCE_NOT_FOUND,
        ] {
            assert!(is_the_callers(callers), "{}", callers.0);
        }
        for servers in [
            ErrorCode::INTERNAL_ERROR,
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
        ] {
            assert!(!is_the_callers(servers), "{}", servers.0);
        }
    }

    /// A `tools/call` rmcp refuses for a tool no host declares, with no
    /// `data`, takes the `NOT_FOUND` class whoever answered it — a lone
    /// host's router, the shared path's, a host written by hand — its code
    /// and message kept; a refusal carrying `data` of its own keeps it, and a
    /// host serving no tools answers the `-32601` every method no host serves
    /// answers.
    #[test]
    fn a_tool_rmcp_refuses_takes_the_not_found_class() {
        let call = Method::from(CallToolRequestMethod::VALUE);
        let tool = || Addressed::Tool("marked".to_owned());

        let refused = tool().said_by_kind(&call, McpError::invalid_params("tool not found", None));
        assert_eq!(refused, tool_not_found());

        let own = McpError::invalid_params(
            "tool not found",
            Some(serde_json::json!({ "reason": "the host's own" })),
        );
        assert_eq!(tool().said_by_kind(&call, own.clone()), own);

        let unserved =
            tool().said_by_kind(&call, McpError::method_not_found::<CallToolRequestMethod>());
        assert_eq!(unserved.code, ErrorCode::METHOD_NOT_FOUND);
        assert_eq!(
            unserved.data,
            Some(serde_json::json!({ "code": "NOT_FOUND" }))
        );
    }

    /// A span is named for its method and the tool or prompt a host declares,
    /// the method alone where none does, and `_OTHER` for a custom method: the
    /// server's values, so no client names a span.
    #[test]
    fn a_span_is_named_for_its_method_and_what_a_host_declares() {
        let named = |method: Method, declared| SpanName::of(&method, declared).to_string();
        let call = || Method::from(CallToolRequestMethod::VALUE);

        assert_eq!(
            named(call(), ServedAddress::tool("forecast")),
            "tools/call forecast"
        );
        assert_eq!(
            named(call(), ServedAddress::tool(operation_log::OTHER)),
            "tools/call"
        );
        assert_eq!(
            named(
                Method::from(GetPromptRequestMethod::VALUE),
                ServedAddress::prompt("outlook")
            ),
            "prompts/get outlook"
        );
        assert_eq!(
            named(
                Method::from(ReadResourceRequestMethod::VALUE),
                ServedAddress::default()
            ),
            "resources/read"
        );
        assert_eq!(
            named(
                Method::from("marked/custom".to_owned()),
                ServedAddress::default()
            ),
            operation_log::OTHER
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

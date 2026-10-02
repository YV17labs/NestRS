//! The wrapper that carries ambient request state across rmcp's spawn.
//!
//! rmcp dispatches every operation on its own spawned task, so a task-local
//! installed around the poem endpoint is gone by the time a handler method
//! runs. [`PropagatingHandler`] re-installs it *inside* the dispatch.
//!
//! # Why this delegates every method
//!
//! Up to rmcp 2.x the whole server surface funnelled through one
//! `Service::handle_request`, and wrapping that single method covered
//! everything. rmcp 3.x moved the dispatch into a blanket
//! `impl<H: ServerHandler> Service<RoleServer> for H` and bounded
//! `StreamableHttpService` on [`ServerHandler`] itself, so the single seam no
//! longer exists: the wrapper has to *be* a `ServerHandler`, and every method
//! it does not delegate silently reverts to rmcp's default (`ListToolsResult`
//! empty, `prompts/get` method-not-found) — the inner handler's own
//! implementation would be dropped, not merely unwrapped.
//!
//! So the delegation below has to stay **exhaustive**, and
//! `#[deny(clippy::missing_trait_methods)]` on the impl is what keeps it that
//! way: a method rmcp adds and this file does not override is a compile error
//! naming it. That gate exists because the obligation is structural and every
//! behavioural proof of it was blind — rmcp 3.3 added `negotiate_initialize`,
//! the integration suite's probe host records only methods someone thought to
//! add to it, and the wrapper silently inherited rmcp's default with every
//! suite green.
//!
//! # What each kind of operation gets
//!
//! | Kind | Request span | Request scope | Guard ability | Data context | Files a line |
//! |---|---|---|---|---|---|
//! | Requests (`tools/call`, `prompts/get`, `resources/read`, `completion/complete`, `logging/setLevel`, `tasks/*`, lifecycle, custom) | yes | yes | yes | yes | yes |
//! | `subscriptions/listen` | yes | yes | yes | no — it outlives any sane transaction | yes |
//! | Notifications | yes | yes | no | no — nothing to commit or roll back on | yes |
//! | Synchronous accessors (`get_info`, `get_tool`, …) | — | — | — | — | no — no work is dispatched |
//!
//! The last column is why notifications are not a special case: they commit
//! nothing and run no guard, but they *are* dispatched work, so an operator asking
//! `nest_rs::operation` what this endpoint did gets them too.
//!
//! The span is in that table for the same reason the scope is: it is ambient
//! request state, and everything dispatched here runs on a task the request did
//! not create. A notification carries it too — it commits nothing, but the
//! events it emits still belong to the request that sent it.
//!
//! # Where an operation ends
//!
//! rmcp spawns every dispatch and never stops or watches one, so this file is
//! where an MCP operation's end is decided, and it has four. It **settles** —
//! `ok` or `error`. It **unwinds** — the panic is contained here, the line
//! files `panic`, and the client is answered with an internal error, since an
//! unwinding task answers nobody and the client would wait out its own
//! timeout. It is **cancelled by its client** — `notifications/cancelled`, or a
//! stateless disconnect. Or the **transport** ends it: a `subscriptions/listen`
//! at the shutdown signal, answered with the final result the schema defines
//! for a graceful server teardown, and anything still running when the window
//! closes, dropped where it waits. The last two file `cancelled`. The line is
//! filed by [`OperationLine`], so an end nobody saw — rmcp dropping the task —
//! still files one.

// `subscribe` / `unsubscribe` are SEP-2575-deprecated in rmcp but still part of
// the trait for legacy protocol versions; a wrapper must forward them or a
// legacy client silently loses the inner handler's implementation.
#![expect(
    deprecated,
    reason = "rmcp still routes the deprecated methods for legacy protocol versions"
)]

use std::borrow::Cow;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures_util::FutureExt;
use nest_rs_core::{Correlation, RequestContinuation, operation_log};
use nest_rs_http::DetachedWork;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResponse, CancelTaskMethod,
    CancelTaskParams, CancelledNotificationMethod, CancelledNotificationParam,
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
use crate::context::{McpAmbient, McpToolContext, OperationOutcome, OperationValue};
use crate::guard::{BoxFuture, McpOperationGuard};

/// Wraps a tool host so each operation runs with the request scope, the
/// operation guard's ambient state, and the registered [`McpToolContext`]'s
/// state installed.
///
/// Built by [`endpoint`](crate::endpoint) around the handler the `#[mcp]`
/// factory produces — never constructed by hand.
pub struct PropagatingHandler<H> {
    inner: H,
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    detached: DetachedWork,
}

impl<H> PropagatingHandler<H> {
    pub(crate) fn new(
        inner: H,
        guard: Arc<dyn McpOperationGuard>,
        context: Option<Arc<dyn McpToolContext>>,
        detached: DetachedWork,
    ) -> Self {
        Self {
            inner,
            guard,
            context,
            detached,
        }
    }

    /// The nesting every wrapped operation shares: the data context (which owns
    /// the operation's transaction) wraps the guard's `around` (which installs
    /// the caller's ability), which wraps the request scope, which wraps the
    /// dispatch. Identical to the GraphQL bridge's order, so the two transports
    /// cannot drift.
    ///
    /// With no data context registered the guard still installs the ability —
    /// the handler then reads through a scoped `Ability` with no executor, which
    /// is `Repo`'s fail-closed case rather than a silently unscoped one.
    ///
    /// # What names the operation
    ///
    /// Two values, because two questions are asked of an MCP line and neither
    /// answers the other. `method` is the **JSON-RPC method** a client
    /// addressed — `tools/call`, `prompts/get` — read off rmcp's own
    /// `ConstString` marker for the request rather than spelled here, so a
    /// protocol rename is a compile error instead of a stale literal. `addressed`
    /// is **which** tool, prompt or resource that method named, and it is the
    /// value that makes the line worth reading: without it every `tools/call` in
    /// a deployment files a byte-identical line, and the one field that
    /// distinguishes them travels in the request the wrapper already holds.
    ///
    /// It is `None` wherever the protocol addresses nothing — `tools/list`,
    /// `ping`, `initialize` — rather than a sentinel: `tracing` drops a `None`
    /// field, so an unaddressed operation files no `operation` key at all, and a
    /// query for one can never match a method that has none.
    ///
    /// # How it ends
    ///
    /// See the module doc. `stopped` resolves when the operation is to end
    /// before it settles — its client cancelled it, or the transport is going
    /// away from a subscription — and yields what the client is answered then.
    /// The transport stopping it at the close of its window
    /// ([`DetachedWork`]) answers a cancellation error. Either way the
    /// operation is dropped where it waits and files
    /// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED).
    async fn dispatch<T, F, S>(
        &self,
        method: &str,
        addressed: Option<&str>,
        ambient: McpAmbient,
        context: Option<&Arc<dyn McpToolContext>>,
        stopped: S,
        inner: F,
    ) -> Result<T, McpError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, McpError>> + Send,
        S: Future<Output = Result<T, McpError>> + Send,
    {
        let McpAmbient {
            scope,
            captured: context_captured,
            guard_captured,
            span,
            correlation,
        } = ambient;

        // **An MCP operation is its own unit of work**, so it opens its own span
        // rather than re-entering the HTTP request's. Under rmcp's default
        // session mode one request carries many operations, and filing them all
        // under the request that opened the session makes "what did this tool
        // call do" unanswerable. The correlation is a `child()`: same trace, a
        // fresh span, the request's span as its parent — the shape `ws.message`
        // already had.
        let correlation = correlation.child();
        // The span carries what the line carries, in the conventions' dotted
        // shape — a span field may be dotted where a line's may not. One
        // `mcp.operation.name` where the semantic conventions have three
        // (`mcp.tool.name`, `mcp.prompt.name`, `mcp.resource.uri`), because this
        // is one seam over every method: which of the three it was is
        // `mcp.method.name`, right beside it.
        let operation = nest_rs_core::operation_span!(
            crate::unit::OPERATION,
            &correlation,
            mcp.method.name = method,
            mcp.operation.name = addressed,
        );
        // The line is filed outside the scope `scoped` installs — where the
        // guard's verdict and the whole duration are known — so it holds the
        // correlation it reports under rather than reading an ambient one, and
        // the span it records the same outcome on.
        let line = OperationLine::open(method, addressed, correlation.clone(), operation.clone());

        // One box, not two: the type erasure the guard's `around` needs and the
        // scope installation are the same future, and this runs on every MCP
        // operation.
        let scoped: BoxFuture<'_, OperationOutcome> = Box::pin(async move {
            nest_rs_core::with_request_scope(scope, correlation, inner)
                .await
                .map(OperationValue::new)
        });

        // Instrumented around the *outermost* composition, so the data context,
        // the guard's `around` and the handler all inherit the request's span
        // rather than only the innermost of them. Composing inside the block
        // matters as much as awaiting inside it: an `around` that opens a span
        // of its own does so when it is called, not when it is polled.
        //
        // A disabled span (nothing installed a subscriber, or no HTTP span is
        // mounted) costs one branch per poll and no allocation, so this is not
        // gated on anything.
        async move {
            let guarded = match &guard_captured {
                Some(captured) => self.guard.around(captured, scoped),
                None => scoped,
            };
            let ran = async move {
                match (context, &context_captured) {
                    (Some(context), Some(captured)) => context.around(captured, guarded).await,
                    _ => guarded.await,
                }
            };
            let ended = self
                .detached
                .run(async move {
                    tokio::select! {
                        biased;
                        ran = AssertUnwindSafe(ran).catch_unwind() => Ended::Ran(ran),
                        answer = stopped => Ended::Stopped(answer),
                    }
                })
                .await;
            match ended {
                Some(Ended::Ran(Ok(Ok(value)))) => {
                    line.file(operation_log::OK);
                    value.take::<T>()
                }
                Some(Ended::Ran(Ok(Err(error)))) => {
                    line.file(operation_log::ERROR);
                    Err(error)
                }
                Some(Ended::Ran(Err(payload))) => {
                    line.unwound(&*payload);
                    Err(McpError::internal_error(
                        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
                        None,
                    ))
                }
                Some(Ended::Stopped(answer)) => {
                    line.file(operation_log::CANCELLED);
                    answer
                }
                None => {
                    line.file(operation_log::CANCELLED);
                    Err(cancelled_before_completion())
                }
            }
        }
        .instrument(operation)
        // Inside the request's span, so the operation stays nested under the
        // request that carried it. That nesting is what `tracing-opentelemetry`
        // builds the exported tree from; the exported parent link is the
        // correlation's, above.
        //
        // **It contributes nothing to a log line**, and must not — see
        // `nest_rs_core::logging::TextFormat`. This is about the exported tree,
        // not about what a line shows.
        .instrument(span)
        .await
    }
}

/// How one dispatched operation ended, before the line names it.
enum Ended<R, A> {
    /// It ran to a result, or unwound.
    Ran(std::thread::Result<R>),
    /// It was stopped first, with the answer the stop gives.
    Stopped(A),
}

/// What rmcp is handed for an operation stopped before it settled: it drops the
/// answer to a request its client cancelled, and a stopped transport has no
/// connection left to carry one, so the sentence is for the log of a client
/// that somehow still reads — never a claim the work was done.
fn cancelled_before_completion() -> McpError {
    McpError::internal_error("the operation was cancelled before it completed", None)
}

/// One operation's `mcp.operation` line, held while the operation runs and
/// filed exactly once — by the end the dispatch saw, or, when the operation is
/// dropped before any end was seen, by `Drop`, as
/// [`CANCELLED`](operation_log::CANCELLED).
///
/// One line per operation. rmcp addresses many operations over one request, so
/// without it a tool call is anonymous on the console — the endpoint's HTTP
/// access line names the session, not the work.
struct OperationLine<'a> {
    method: &'a str,
    addressed: Option<&'a str>,
    /// Entered to file the line: it sits outside the scope the operation
    /// installs, and a line with no ambient context carries no ids at all —
    /// which is exactly what it did until a capture of real output showed the
    /// gap.
    correlation: Correlation,
    /// The operation's own span, which records the outcome the line files.
    /// [`Span::none`](tracing::Span::none) for a notification, which opens no
    /// span of its own: the span it runs under is the HTTP request's, whose
    /// outcome is the request's to record.
    span: tracing::Span,
    started: std::time::Instant,
    filed: bool,
}

impl<'a> OperationLine<'a> {
    fn open(
        method: &'a str,
        addressed: Option<&'a str>,
        correlation: Correlation,
        span: tracing::Span,
    ) -> Self {
        Self {
            method,
            addressed,
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
            tracing::error!(
                target: crate::TARGET,
                method = self.method,
                operation = self.addressed,
                panic = nest_rs_core::panic_message(payload),
                "mcp operation panicked; its client is answered with an internal error",
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
                // The JSON-RPC method this operation was, and the tool,
                // prompt or resource it named. Flat, never dotted: a dotted
                // field name is ambiguous to `tracing`'s parser beside a
                // path target. `operation` is absent where the protocol
                // addressed nothing — a notification included.
                method = self.method,
                operation = self.addressed,
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

/// Delegate one request-shaped method: read the ambient state off the
/// operation's context, then run the inner handler inside the full nesting.
///
/// Written as a macro because the 18 request methods differ only in their
/// parameters and result type — spelling each body out would invite exactly the
/// per-method drift this wrapper exists to prevent.
///
/// **Every invocation states both names**, and that is the point of the single
/// arm: `$method` is what a client addressed, `$addressed` is what the request
/// named inside it or `None` where the protocol names nothing. A method that
/// simply forgot to say would have to say `None` out loud, which is a line a
/// reviewer sees. Both are read **before** the parameters are handed to the
/// inner handler, since that call consumes them.
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
                let ambient = McpAmbient::from_extensions(&context.extensions).unwrap_or_default();
                let method: Cow<'_, str> = ($method).into();
                let addressed: Option<String> = $addressed;
                let cancelled = context.ct.clone().cancelled_owned();
                self.dispatch(
                    &method,
                    addressed.as_deref(),
                    ambient,
                    self.context.as_ref(),
                    async move {
                        cancelled.await;
                        Err(cancelled_before_completion())
                    },
                    self.inner.$name($($arg,)* context),
                )
                .await
            }
        }
    };
}

/// Delegate one notification: the request span and scope are installed — the
/// first so the notification's events are attributable, the second so
/// `Scoped<T>` resolves uniformly — and nothing else is. See the table in the
/// module doc.
macro_rules! notification_method {
    ($name:ident ( $($arg:ident : $arg_ty:ty),* ), $method:expr $(,)?) => {
        fn $name(
            &self,
            $($arg: $arg_ty,)*
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            async move {
                let McpAmbient { scope, span, correlation, .. } =
                    McpAmbient::from_extensions(&context.extensions).unwrap_or_default();
                let method: Cow<'_, str> = ($method).into();
                // A notification is dispatched work, so it files the family's
                // line like every other unit — the same message as a request
                // method, discriminated by `method`, because a notification *is*
                // an MCP operation and one query should find both. It addresses
                // no tool, prompt or resource, so its `operation` is absent
                // rather than empty, for the reason it is absent on `tools/list`.
                //
                // Not recorded on a span: a notification opens none of its own,
                // and the span it runs under is the HTTP request's, whose outcome
                // is the request's to record.
                let line = OperationLine::open(&method, None, correlation.clone(), tracing::Span::none());
                let handled =
                    AssertUnwindSafe(self.inner.$name($($arg,)* context)).catch_unwind();
                nest_rs_core::with_request_scope(scope, correlation, async move {
                    // rmcp spawns a notification's handler as it does a
                    // request's, so it too is stopped with the transport rather
                    // than left running on through the shutdown hooks. A client
                    // cannot cancel a notification: it has no id to name.
                    //
                    // `ok` when it ran to its end, and that is honest rather than
                    // assumed: a notification handler returns `()`, so it has no
                    // failure channel — only an unwind, contained here, since a
                    // notification has no answer for an unwinding task to lose.
                    match self.detached.run(handled).await {
                        Some(Ok(())) => line.file(operation_log::OK),
                        Some(Err(payload)) => line.unwound(&*payload),
                        None => line.file(operation_log::CANCELLED),
                    }
                })
                .instrument(span)
                .await
            }
        }
    };
}

#[deny(clippy::missing_trait_methods)]
impl<H: ServerHandler> ServerHandler for PropagatingHandler<H> {
    // --- lifecycle & discovery ---------------------------------------------
    request_method!(ping() -> (), PingRequestMethod::VALUE, None);
    request_method!(
        initialize(request: InitializeRequestParams) -> InitializeResult,
        InitializeResultMethod::VALUE,
        None,
    );
    request_method!(discover() -> DiscoverResult, DiscoverRequestMethod::VALUE, None);

    // --- tools --------------------------------------------------------------
    request_method!(
        call_tool(request: CallToolRequestParams) -> CallToolResponse,
        CallToolRequestMethod::VALUE,
        Some(request.name.to_string()),
    );
    request_method!(
        list_tools(request: Option<PaginatedRequestParams>) -> ListToolsResult,
        ListToolsRequestMethod::VALUE,
        None,
    );

    // --- prompts ------------------------------------------------------------
    request_method!(
        get_prompt(request: GetPromptRequestParams) -> GetPromptResponse,
        GetPromptRequestMethod::VALUE,
        Some(request.name.clone()),
    );
    request_method!(
        list_prompts(request: Option<PaginatedRequestParams>) -> ListPromptsResult,
        ListPromptsRequestMethod::VALUE,
        None,
    );

    // --- resources ----------------------------------------------------------
    request_method!(
        read_resource(request: ReadResourceRequestParams) -> ReadResourceResponse,
        ReadResourceRequestMethod::VALUE,
        Some(request.uri.clone()),
    );
    request_method!(
        list_resources(request: Option<PaginatedRequestParams>) -> ListResourcesResult,
        ListResourcesRequestMethod::VALUE,
        None,
    );
    request_method!(
        list_resource_templates(request: Option<PaginatedRequestParams>)
            -> ListResourceTemplatesResult,
        ListResourceTemplatesRequestMethod::VALUE,
        None,
    );
    request_method!(
        subscribe(request: SubscribeRequestParams) -> (),
        SubscribeRequestMethod::VALUE,
        Some(request.uri.clone()),
    );
    request_method!(
        unsubscribe(request: UnsubscribeRequestParams) -> (),
        UnsubscribeRequestMethod::VALUE,
        Some(request.uri.clone()),
    );

    // --- completion & logging -----------------------------------------------
    request_method!(
        complete(request: CompleteRequestParams) -> CompleteResult,
        CompleteRequestMethod::VALUE,
        // A completion addresses the prompt or resource template it is
        // completing an argument *of*, which is the operation an operator reads
        // — the argument's own name is a field of that operation, not its
        // identity. `Reference` is `#[non_exhaustive]`, so a variant rmcp adds
        // later addresses something this crate has never seen and says so by
        // filing no name rather than a wrong one.
        match &request.r#ref {
            Reference::Prompt(prompt) => Some(prompt.name.clone()),
            Reference::Resource(resource) => Some(resource.uri.clone()),
            _ => None,
        },
    );
    request_method!(
        set_level(request: SetLevelRequestParams) -> (),
        SetLevelRequestMethod::VALUE,
        // A logging level is a setting, not something addressed: there is no
        // tool, prompt or resource here to name.
        None,
    );

    // --- tasks (SEP-2663) ----------------------------------------------------
    request_method!(
        get_task(request: GetTaskParams) -> GetTaskResult,
        GetTaskMethod::VALUE,
        Some(request.task_id.clone()),
    );
    request_method!(
        update_task(request: UpdateTaskParams) -> (),
        UpdateTaskMethod::VALUE,
        Some(request.task_id.clone()),
    );
    request_method!(
        cancel_task(request: CancelTaskParams) -> (),
        CancelTaskMethod::VALUE,
        Some(request.task_id.clone()),
    );

    // --- custom methods ------------------------------------------------------
    // The one request whose protocol method is **data**: a custom method carries
    // its own name on the wire, so there is no `ConstString` to read it from and
    // the value itself is what a client addressed.
    request_method!(
        on_custom_request(request: CustomRequest) -> CustomResult,
        request.method.clone(),
        None,
    );

    // --- notifications -------------------------------------------------------
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

    /// `subscriptions/listen` (SEP-2575) runs until the subscription is
    /// cancelled, so it takes the request scope and the guard's ability but
    /// **not** the data context: a transaction held open for the life of a
    /// subscription would pin a pooled connection for the same duration.
    ///
    /// Its client's cancellation is how it ends normally — the host's own
    /// `listen` answers it, and files the `ok` it is. The other end is the
    /// transport going away: a subscription has no end of its own, so at the
    /// shutdown signal it is answered with `Ok(())`, which rmcp sends as the
    /// final `SubscriptionsListenResult` the 2026-07-28 schema defines for a
    /// graceful server teardown, and files `cancelled` — the server, not the
    /// subscriber, ended it.
    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let ambient =
            McpAmbient::from_extensions(&context.request_context().extensions).unwrap_or_default();
        let going_away = self.detached.going_away();
        self.dispatch(
            SubscriptionsListenRequestMethod::VALUE,
            None,
            ambient,
            None,
            async move {
                going_away.await;
                Ok(())
            },
            self.inner.listen(context),
        )
        .await
    }

    // --- synchronous accessors ------------------------------------------------
    // No ambient state to install: these are pure reads of the inner handler's
    // own declaration, called by rmcp outside any operation dispatch.

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

    /// Delegated for the reason rmcp's own `impl_server_handler_for_wrapper!`
    /// delegates it: a host that narrows its supported versions, or overrides
    /// this outright to refuse one, has that answer dropped if the wrapper
    /// rebuilds it from the defaults instead of asking.
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
    use crate::guards::AllowAllMcpGuard;

    /// Deliberately not `ProtocolVersion::LATEST`, which is what
    /// `InitializeRequestParams::default()` carries: pinning this to `LATEST`
    /// makes a wrapper that substitutes a request of its own invisible.
    const THE_CALLER_ASKS_FOR: ProtocolVersion = ProtocolVersion::V_2025_06_18;

    /// Not the version asked for, and not one any default arrives at, so a
    /// wrapper that overwrites the host's verdict is visible too.
    const THE_HOST_ANSWERS_WITH: ProtocolVersion = ProtocolVersion::V_2024_11_05;

    /// A host that *overrides* negotiation, and asserts on its own input —
    /// the two ways a delegation goes wrong are invisible from the result
    /// alone.
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

    /// The lint on the impl proves the method is *present*; this proves it
    /// reaches the host with the caller's own request and returns the host's
    /// own answer. It is a direct call because rmcp reaches
    /// `negotiate_initialize` through the `initialize` this wrapper already
    /// delegates, so no wire probe can tell a delegated call from an inherited
    /// one.
    #[test]
    fn the_wrapper_asks_the_inner_host_to_negotiate() {
        let wrapper = PropagatingHandler::new(
            NegotiatingHost,
            Arc::new(AllowAllMcpGuard),
            None,
            DetachedWork::new(),
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

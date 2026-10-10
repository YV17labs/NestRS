//! The wrapper that carries ambient request state across rmcp's spawn.
//!
//! rmcp dispatches every operation on its own task, where a task-local installed
//! around the poem endpoint is gone; [`PropagatingHandler`] re-installs it inside
//! the dispatch. A method it does not delegate silently reverts to rmcp's default,
//! so the impl carries `#[deny(clippy::missing_trait_methods)]`.
//!
//! | Kind | Request span | Request scope | Guard ability | Data context | Files a line |
//! |---|---|---|---|---|---|
//! | Requests (`tools/call`, `prompts/get`, `resources/read`, `completion/complete`, `logging/setLevel`, `tasks/*`, lifecycle, custom) | yes | yes | yes | yes | yes |
//! | `subscriptions/listen` | yes | yes | yes | no — it outlives any sane transaction | yes |
//! | Notifications | yes | yes | no | no — nothing to commit or roll back on | yes |
//! | Synchronous accessors (`get_info`, `get_tool`, …) | — | — | — | — | no — no work is dispatched |
//!
//! An operation files `ok` or `error`; `panic` when it unwinds (contained, the
//! client answered with an internal error rather than left to its timeout); or
//! `cancelled` when its client cancels it or the transport stops it. The line is
//! filed by [`OperationLine`], so an end nobody saw still files one.

#![expect(
    deprecated,
    reason = "rmcp still routes the deprecated methods for legacy protocol versions"
)]

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

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
/// Built by [`endpoint`](fn@crate::endpoint), never by hand.
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

    /// The nesting every wrapped operation shares, outermost first: the data
    /// context (the transaction), the guard's `around` (the ability), the request
    /// scope, the dispatch — the GraphQL bridge's order.
    ///
    /// `method` is the JSON-RPC method, `addressed` the tool, prompt or resource
    /// it named, `None` where the protocol names nothing. `stopped` resolves when
    /// the operation is to end before it settles, with what the client is answered.
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

        // Its own unit of work: one session request carries many operations.
        let correlation = correlation.child();
        // One `mcp.operation.name` for the conventions' three (`mcp.tool.name`,
        // `mcp.prompt.name`, `mcp.resource.uri`); `mcp.method.name` says which.
        let operation = nest_rs_core::operation_span!(
            crate::unit::OPERATION,
            &correlation,
            mcp.method.name = method,
            mcp.operation.name = addressed,
        );
        // Filed outside the scope `scoped` installs, so it holds its own correlation.
        let line = OperationLine::open(method, addressed, correlation.clone(), operation.clone());

        let scoped: BoxFuture<'_, OperationOutcome> = Box::pin(async move {
            nest_rs_core::with_request_scope(scope, correlation, inner)
                .await
                .map(OperationValue::new)
        });

        // Composed inside the instrumented block: an `around` opening a span of
        // its own does so when called, not when polled.
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
                        ran = nest_rs_core::panic::contain(ran) => Ended::Ran(ran),
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
        // For the exported OTel tree only; it must add nothing to a log line
        // (`nest_rs_core::logging::TextFormat`).
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

/// What rmcp is handed for an operation stopped before it settled — never a
/// claim the work was done.
fn cancelled_before_completion() -> McpError {
    McpError::internal_error("the operation was cancelled before it completed", None)
}

/// One operation's `mcp.operation` line, filed exactly once — by the end the
/// dispatch saw, else by `Drop` as [`CANCELLED`](operation_log::CANCELLED).
struct OperationLine<'a> {
    method: &'a str,
    addressed: Option<&'a str>,
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
            nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "mcp operation panicked; its client is answered with an internal error",
                method = self.method,
                operation = self.addressed,
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

/// Delegate one request-shaped method inside the full nesting. Every invocation
/// states `$addressed`, `None` out loud; it is read before the call consumes the parameters.
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

/// Delegate one notification: the request span and scope are installed, and
/// nothing else is.
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
                let line = OperationLine::open(&method, None, correlation.clone(), tracing::Span::none());
                let handled = nest_rs_core::panic::contain(self.inner.$name($($arg,)* context));
                nest_rs_core::with_request_scope(scope, correlation, async move {
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
    request_method!(ping() -> (), PingRequestMethod::VALUE, None);
    request_method!(
        initialize(request: InitializeRequestParams) -> InitializeResult,
        InitializeResultMethod::VALUE,
        None,
    );
    request_method!(discover() -> DiscoverResult, DiscoverRequestMethod::VALUE, None);

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

    request_method!(
        complete(request: CompleteRequestParams) -> CompleteResult,
        CompleteRequestMethod::VALUE,
        // `Reference` is `#[non_exhaustive]`: a variant rmcp adds files no name.
        match &request.r#ref {
            Reference::Prompt(prompt) => Some(prompt.name.clone()),
            Reference::Resource(resource) => Some(resource.uri.clone()),
            _ => None,
        },
    );
    request_method!(
        set_level(request: SetLevelRequestParams) -> (),
        SetLevelRequestMethod::VALUE,
        None,
    );

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

    request_method!(
        on_custom_request(request: CustomRequest) -> CustomResult,
        request.method.clone(),
        None,
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

    /// No data context: a transaction would pin a pooled connection for the
    /// subscription's life. At the shutdown signal it answers `Ok(())`, which rmcp
    /// sends as the schema's final result for a graceful teardown, and files `cancelled`.
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
    use crate::guards::AllowAllMcpGuard;

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

    /// A direct call: rmcp reaches this through `initialize`, so no wire probe
    /// tells a delegated call from an inherited one.
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

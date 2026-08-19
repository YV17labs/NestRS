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
//! So the delegation below is **exhaustive by construction, and must stay that
//! way**: rmcp's `ServerHandler` is the list, `propagate.rs` in the
//! integration suite is the proof. Adding a capability to rmcp means adding it
//! here, otherwise the framework quietly answers for the tool host.
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

// `subscribe` / `unsubscribe` are SEP-2575-deprecated in rmcp but still part of
// the trait for legacy protocol versions; a wrapper must forward them or a
// legacy client silently loses the inner handler's implementation.
#![expect(deprecated)]

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

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
    RootsListChangedNotificationMethod, ServerInfo, SetLevelRequestMethod, SetLevelRequestParams,
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
}

impl<H> PropagatingHandler<H> {
    pub(crate) fn new(
        inner: H,
        guard: Arc<dyn McpOperationGuard>,
        context: Option<Arc<dyn McpToolContext>>,
    ) -> Self {
        Self {
            inner,
            guard,
            context,
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
    async fn dispatch<T, F>(
        &self,
        method: &str,
        addressed: Option<&str>,
        ambient: McpAmbient,
        context: Option<&Arc<dyn McpToolContext>>,
        inner: F,
    ) -> Result<T, McpError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, McpError>> + Send,
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
        // Held for the line below: the ambient context is installed *inside*
        // `scoped`, so the outer composition — where the guard's verdict and the
        // whole duration are known — is outside it.
        let reported = correlation.clone();
        // The span carries what the line carries, in the conventions' dotted
        // shape — a span field may be dotted where a line's may not. One
        // `mcp.operation.name` where the semantic conventions have three
        // (`mcp.tool.name`, `mcp.prompt.name`, `mcp.resource.uri`), because this
        // is one seam over every method: which of the three it was is
        // `mcp.method.name`, right beside it.
        let operation = nest_rs_core::operation_span!(
            target: crate::TARGET,
            kind: nest_rs_core::operation_log::kind::SERVER,
            crate::unit::OPERATION,
            &correlation,
            mcp.method.name = method,
            mcp.operation.name = addressed,
        );

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
        let started = std::time::Instant::now();
        async move {
            let guarded = match &guard_captured {
                Some(captured) => self.guard.around(captured, scoped),
                None => scoped,
            };

            let settled = match (context, &context_captured) {
                (Some(context), Some(captured)) => context.around(captured, guarded).await,
                _ => guarded.await,
            };
            // One line per operation. rmcp addresses many operations over one
            // request, so without it a tool call is anonymous on the console —
            // the endpoint's HTTP access line names the session, not the work.
            //
            // Emitted through the correlation explicitly, because this sits
            // *outside* the scope `scoped` installs and a line with no ambient
            // context carries no ids at all — which is exactly what it did until a
            // capture of real output showed the gap. Outside is still the right
            // depth: here the guard's verdict and the whole duration are known.
            nest_rs_core::RequestContinuation::new(None, reported).enter(|| {
                tracing::info!(
                    name: crate::unit::OPERATION,
                    target: nest_rs_core::operation_log::TARGET,
                    message = crate::unit::OPERATION,
                    // The JSON-RPC method this operation was, and the tool,
                    // prompt or resource it named. Flat, never dotted: a dotted
                    // field name is ambiguous to `tracing`'s parser beside a
                    // path target. `operation` is absent where the protocol
                    // addressed nothing.
                    method = method,
                    operation = addressed,
                    outcome = if settled.is_ok() {
                        nest_rs_core::operation_log::OK
                    } else {
                        nest_rs_core::operation_log::ERROR
                    },
                    duration_ms = nest_rs_core::operation_log::duration_ms(started),
                );
            });
            settled
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
        .and_then(OperationValue::take::<T>)
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
                self.dispatch(
                    &method,
                    addressed.as_deref(),
                    ambient,
                    self.context.as_ref(),
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
                let started = std::time::Instant::now();
                let method: Cow<'_, str> = ($method).into();
                nest_rs_core::with_request_scope(scope, correlation, async move {
                    self.inner.$name($($arg,)* context).await;
                    // A notification is dispatched work, so it files the family's
                    // line like every other unit — the same message as a request
                    // method, discriminated by `method`, because a notification
                    // *is* an MCP operation and one query should find both.
                    //
                    // No `operation`: a notification addresses no tool, prompt or
                    // resource, and the field is absent rather than empty for the
                    // same reason it is absent on `tools/list`.
                    //
                    // `outcome` is `ok` and that is honest rather than assumed: a
                    // notification handler returns `()`, so it has no failure
                    // channel, and had it unwound this line would not be reached.
                    tracing::info!(
                        name: crate::unit::OPERATION,
                        target: nest_rs_core::operation_log::TARGET,
                        message = crate::unit::OPERATION,
                        method = %method,
                        outcome = nest_rs_core::operation_log::OK,
                        duration_ms = nest_rs_core::operation_log::duration_ms(started),
                    );
                })
                .instrument(span)
                .await
            }
        }
    };
}

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
    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let ambient =
            McpAmbient::from_extensions(&context.request_context().extensions).unwrap_or_default();
        self.dispatch(
            SubscriptionsListenRequestMethod::VALUE,
            None,
            ambient,
            None,
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

    fn get_info(&self) -> ServerInfo {
        self.inner.get_info()
    }
}

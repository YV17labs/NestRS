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
//! | Kind | Request span | Request scope | Guard ability | Data context |
//! |---|---|---|---|---|
//! | Requests (`tools/call`, `prompts/get`, `resources/read`, `completion/complete`, `logging/setLevel`, `tasks/*`, lifecycle, custom) | yes | yes | yes | yes |
//! | `subscriptions/listen` | yes | yes | yes | no — it outlives any sane transaction |
//! | Notifications | yes | yes | no | no — nothing to commit or roll back on |
//! | Synchronous accessors (`get_info`, `get_tool`, …) | — | — | — | — |
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
    CallToolRequestParams, CallToolResponse, CancelTaskParams, CancelledNotificationParam,
    CompleteRequestParams, CompleteResult, CustomNotification, CustomRequest, CustomResult,
    DiscoverResult, GetPromptRequestParams, GetPromptResponse, GetTaskParams, GetTaskResult,
    InitializeRequestParams, InitializeResult, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ServerInfo,
    SetLevelRequestParams, SubscribeRequestParams, SubscriptionFilter, Tool,
    UnsubscribeRequestParams, UpdateTaskParams,
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
    async fn dispatch<T, F>(
        &self,
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
        let operation = nest_rs_core::operation_span!(
            target: "nest_rs::mcp",
            kind: "server",
            "mcp.operation",
            &correlation,
        );

        // One box, not two: the type erasure the guard's `around` needs and the
        // scope installation are the same future, and this runs on every MCP
        // operation.
        let scoped: BoxFuture<'_, OperationOutcome> = Box::pin(async move {
            nest_rs_core::with_request_scope(scope, correlation, None, inner)
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

            match (context, &context_captured) {
                (Some(context), Some(captured)) => context.around(captured, guarded).await,
                _ => guarded.await,
            }
        }
        .instrument(operation)
        // Inside the request's span, so the console still shows the operation
        // nested under the request that carried it; the exported parent link is
        // the correlation's, above.
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
macro_rules! request_method {
    ($name:ident ( $($arg:ident : $arg_ty:ty),* ) -> $out:ty) => {
        fn $name(
            &self,
            $($arg: $arg_ty,)*
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<$out, McpError>> + MaybeSendFuture + '_ {
            async move {
                let ambient = McpAmbient::from_extensions(&context.extensions).unwrap_or_default();
                self.dispatch(
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
    ($name:ident ( $($arg:ident : $arg_ty:ty),* )) => {
        fn $name(
            &self,
            $($arg: $arg_ty,)*
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            async move {
                let McpAmbient { scope, span, correlation, .. } =
                    McpAmbient::from_extensions(&context.extensions).unwrap_or_default();
                nest_rs_core::with_request_scope(scope, correlation, None, self.inner.$name($($arg,)* context))
                    .instrument(span)
                    .await
            }
        }
    };
}

impl<H: ServerHandler> ServerHandler for PropagatingHandler<H> {
    // --- lifecycle & discovery ---------------------------------------------
    request_method!(ping() -> ());
    request_method!(initialize(request: InitializeRequestParams) -> InitializeResult);
    request_method!(discover() -> DiscoverResult);

    // --- tools --------------------------------------------------------------
    request_method!(call_tool(request: CallToolRequestParams) -> CallToolResponse);
    request_method!(list_tools(request: Option<PaginatedRequestParams>) -> ListToolsResult);

    // --- prompts ------------------------------------------------------------
    request_method!(get_prompt(request: GetPromptRequestParams) -> GetPromptResponse);
    request_method!(list_prompts(request: Option<PaginatedRequestParams>) -> ListPromptsResult);

    // --- resources ----------------------------------------------------------
    request_method!(read_resource(request: ReadResourceRequestParams) -> ReadResourceResponse);
    request_method!(list_resources(request: Option<PaginatedRequestParams>) -> ListResourcesResult);
    request_method!(
        list_resource_templates(request: Option<PaginatedRequestParams>)
            -> ListResourceTemplatesResult
    );
    request_method!(subscribe(request: SubscribeRequestParams) -> ());
    request_method!(unsubscribe(request: UnsubscribeRequestParams) -> ());

    // --- completion & logging -----------------------------------------------
    request_method!(complete(request: CompleteRequestParams) -> CompleteResult);
    request_method!(set_level(request: SetLevelRequestParams) -> ());

    // --- tasks (SEP-2663) ----------------------------------------------------
    request_method!(get_task(request: GetTaskParams) -> GetTaskResult);
    request_method!(update_task(request: UpdateTaskParams) -> ());
    request_method!(cancel_task(request: CancelTaskParams) -> ());

    // --- custom methods ------------------------------------------------------
    request_method!(on_custom_request(request: CustomRequest) -> CustomResult);

    // --- notifications -------------------------------------------------------
    notification_method!(on_cancelled(notification: CancelledNotificationParam));
    notification_method!(on_progress(notification: ProgressNotificationParam));
    notification_method!(on_initialized());
    notification_method!(on_roots_list_changed());
    notification_method!(on_custom_notification(notification: CustomNotification));

    /// `subscriptions/listen` (SEP-2575) runs until the subscription is
    /// cancelled, so it takes the request scope and the guard's ability but
    /// **not** the data context: a transaction held open for the life of a
    /// subscription would pin a pooled connection for the same duration.
    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let ambient =
            McpAmbient::from_extensions(&context.request_context().extensions).unwrap_or_default();
        self.dispatch(ambient, None, self.inner.listen(context))
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

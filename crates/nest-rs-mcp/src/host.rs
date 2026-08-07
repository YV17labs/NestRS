//! [`McpHost`] — one MCP host, seen through a `dyn`-compatible view.
//!
//! [`ServerHandler`] is not object-safe: every method returns an opaque
//! `impl Future`, and `Sized` is a supertrait. A mount that merges several
//! hosts has to hold them behind one pointer type, so this module restates the
//! trait with boxed futures and blanket-implements it for every
//! `ServerHandler`. Same shim [`GraphqlResolverObject`-style composition uses
//! next door, for the same reason.
//!
//! **The surface is exhaustive by construction, and must stay that way.** A
//! method missing here is a method [`CompositeHandler`](crate::CompositeHandler)
//! cannot delegate, which means rmcp's *default* answers for the host — an
//! empty `tools/list`, a `-32601` on `prompts/get` — silently, and only on the
//! wire. rmcp's `ServerHandler` is the list;
//! `tests/integration/propagate.rs` is the proof for the wrapper and
//! `tests/integration/registry.rs` for the merge.
//!
//! [`GraphqlResolverObject`]: https://docs.rs/nest-rs-graphql

// `subscribe` / `unsubscribe` are SEP-2575-deprecated in rmcp but still part of
// the trait for legacy protocol versions; a view that drops them drops real
// traffic.
#![expect(deprecated)]

use std::borrow::Cow;

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
use rmcp::service::{NotificationContext, RequestContext, RoleServer, SubscriptionContext};

use crate::McpError;
use crate::guard::BoxFuture;

/// Restate `ServerHandler` with boxed futures, and blanket-implement it.
///
/// Written as one macro over the method list because the alternative — 24
/// hand-written pairs — is 24 chances for the trait and its blanket impl to
/// drift apart. Every method rmcp's trait has goes in one of the three lists;
/// only `listen` stays hand-written, because its context type is neither a
/// `RequestContext` nor a `NotificationContext`.
macro_rules! dyn_host {
    (
        requests: [ $( $rname:ident ( $($ra:ident : $rty:ty),* ) -> $rout:ty ),* $(,)? ],
        notifications: [ $( $nname:ident ( $($na:ident : $nty:ty),* ) ),* $(,)? ],
        accessors: [ $( $doc:literal $sname:ident ( $($sa:ident : $sty:ty),* ) -> $sout:ty ),* $(,)? ] $(,)?
    ) => {
        /// Object-safe view of an MCP host, so a mount can hold several of them.
        ///
        /// Blanket-implemented for every [`ServerHandler`] — a `#[mcp]` host
        /// never names this trait, it just is one.
        pub trait McpHost: Send + Sync + 'static {
            $(
                /// Delegates to the host's [`ServerHandler`] method of the same
                /// name.
                fn $rname<'a>(
                    &'a self,
                    $($ra: $rty,)*
                    context: RequestContext<RoleServer>,
                ) -> BoxFuture<'a, Result<$rout, McpError>>;
            )*
            $(
                /// Delegates to the host's [`ServerHandler`] notification of the
                /// same name.
                fn $nname<'a>(
                    &'a self,
                    $($na: $nty,)*
                    context: NotificationContext<RoleServer>,
                ) -> BoxFuture<'a, ()>;
            )*

            $(
                #[doc = $doc]
                fn $sname(&self, $($sa: $sty),*) -> $sout;
            )*

            /// Run one established subscription (`subscriptions/listen`).
            fn listen<'a>(
                &'a self,
                context: SubscriptionContext,
            ) -> BoxFuture<'a, Result<(), McpError>>;
        }

        impl<H: ServerHandler> McpHost for H {
            $(
                fn $rname<'a>(
                    &'a self,
                    $($ra: $rty,)*
                    context: RequestContext<RoleServer>,
                ) -> BoxFuture<'a, Result<$rout, McpError>> {
                    Box::pin(<H as ServerHandler>::$rname(self, $($ra,)* context))
                }
            )*
            $(
                fn $nname<'a>(
                    &'a self,
                    $($na: $nty,)*
                    context: NotificationContext<RoleServer>,
                ) -> BoxFuture<'a, ()> {
                    Box::pin(<H as ServerHandler>::$nname(self, $($na,)* context))
                }
            )*

            $(
                fn $sname(&self, $($sa: $sty),*) -> $sout {
                    <H as ServerHandler>::$sname(self, $($sa),*)
                }
            )*

            fn listen<'a>(
                &'a self,
                context: SubscriptionContext,
            ) -> BoxFuture<'a, Result<(), McpError>> {
                Box::pin(<H as ServerHandler>::listen(self, context))
            }
        }
    };
}

dyn_host! {
    requests: [
        // --- lifecycle & discovery ---
        ping() -> (),
        initialize(request: InitializeRequestParams) -> InitializeResult,
        discover() -> DiscoverResult,
        // --- tools ---
        call_tool(request: CallToolRequestParams) -> CallToolResponse,
        list_tools(request: Option<PaginatedRequestParams>) -> ListToolsResult,
        // --- prompts ---
        get_prompt(request: GetPromptRequestParams) -> GetPromptResponse,
        list_prompts(request: Option<PaginatedRequestParams>) -> ListPromptsResult,
        // --- resources ---
        read_resource(request: ReadResourceRequestParams) -> ReadResourceResponse,
        list_resources(request: Option<PaginatedRequestParams>) -> ListResourcesResult,
        list_resource_templates(request: Option<PaginatedRequestParams>)
            -> ListResourceTemplatesResult,
        subscribe(request: SubscribeRequestParams) -> (),
        unsubscribe(request: UnsubscribeRequestParams) -> (),
        // --- completion & logging ---
        complete(request: CompleteRequestParams) -> CompleteResult,
        set_level(request: SetLevelRequestParams) -> (),
        // --- tasks (SEP-2663) ---
        get_task(request: GetTaskParams) -> GetTaskResult,
        update_task(request: UpdateTaskParams) -> (),
        cancel_task(request: CancelTaskParams) -> (),
        // --- custom methods ---
        on_custom_request(request: CustomRequest) -> CustomResult,
    ],
    notifications: [
        on_cancelled(notification: CancelledNotificationParam),
        on_progress(notification: ProgressNotificationParam),
        on_initialized(),
        on_roots_list_changed(),
        on_custom_notification(notification: CustomNotification),
    ],
    // Synchronous reads of the host's own declaration — no dispatch, no future.
    accessors: [
        "The subset of `requested` this host accepts, `None` when it serves no \
         subscriptions."
        accepted_subscription_filter(requested: &SubscriptionFilter) -> Option<SubscriptionFilter>,
        "This host's definition of `name`, `None` when it serves no such tool."
        get_tool(name: &str) -> Option<Tool>,
        "The protocol versions this host implements."
        supported_protocol_versions() -> Cow<'static, [ProtocolVersion]>,
        "This host's declared capabilities and instructions."
        get_info() -> ServerInfo,
    ],
}

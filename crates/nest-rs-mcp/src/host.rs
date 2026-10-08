//! [`McpHost`] — one MCP host, seen through a `dyn`-compatible view of rmcp's
//! [`ServerHandler`], which is not object-safe.
//!
//! A method missing here would answer with rmcp's default, silently; a method rmcp
//! adds fails `CompositeHandler`'s `#[deny(clippy::missing_trait_methods)]` until added.

#![expect(
    deprecated,
    reason = "rmcp still routes the deprecated methods for legacy protocol versions"
)]

use std::borrow::Cow;

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CancelTaskParams, CancelledNotificationParam,
    CompleteRequestParams, CompleteResult, CustomNotification, CustomRequest, CustomResult,
    DiscoverResult, GetPromptRequestParams, GetPromptResponse, GetTaskParams, GetTaskResult,
    InitializeRequestParams, InitializeResult, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ServerConfig,
    SetLevelRequestParams, SubscribeRequestParams, SubscriptionFilter, Tool,
    UnsubscribeRequestParams, UpdateTaskParams,
};
use rmcp::service::{NotificationContext, RequestContext, RoleServer, SubscriptionContext};

use crate::McpError;
use crate::guard::BoxFuture;

/// Restate `ServerHandler` with boxed futures, and blanket-implement it; `listen`
/// is hand-written, its context neither a `RequestContext` nor a `NotificationContext`.
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
        ping() -> (),
        initialize(request: InitializeRequestParams) -> InitializeResult,
        discover() -> DiscoverResult,
        call_tool(request: CallToolRequestParams) -> CallToolResponse,
        list_tools(request: Option<PaginatedRequestParams>) -> ListToolsResult,
        get_prompt(request: GetPromptRequestParams) -> GetPromptResponse,
        list_prompts(request: Option<PaginatedRequestParams>) -> ListPromptsResult,
        read_resource(request: ReadResourceRequestParams) -> ReadResourceResponse,
        list_resources(request: Option<PaginatedRequestParams>) -> ListResourcesResult,
        list_resource_templates(request: Option<PaginatedRequestParams>)
            -> ListResourceTemplatesResult,
        subscribe(request: SubscribeRequestParams) -> (),
        unsubscribe(request: UnsubscribeRequestParams) -> (),
        complete(request: CompleteRequestParams) -> CompleteResult,
        set_level(request: SetLevelRequestParams) -> (),
        get_task(request: GetTaskParams) -> GetTaskResult,
        update_task(request: UpdateTaskParams) -> (),
        cancel_task(request: CancelTaskParams) -> (),
        on_custom_request(request: CustomRequest) -> CustomResult,
    ],
    notifications: [
        on_cancelled(notification: CancelledNotificationParam),
        on_progress(notification: ProgressNotificationParam),
        on_initialized(),
        on_roots_list_changed(),
        on_custom_notification(notification: CustomNotification),
    ],
    accessors: [
        "The subset of `requested` this host accepts, `None` when it serves no \
         subscriptions."
        accepted_subscription_filter(requested: &SubscriptionFilter) -> Option<SubscriptionFilter>,
        "This host's definition of `name`, `None` when it serves no such tool."
        get_tool(name: &str) -> Option<Tool>,
        "The protocol versions this host implements."
        supported_protocol_versions() -> Cow<'static, [ProtocolVersion]>,
        "This host's declared capabilities and instructions."
        get_info() -> ServerConfig,
        "The `initialize` result this host negotiates for `request`, derived \
         from the two accessors above unless the host overrides it."
        negotiate_initialize(request: &InitializeRequestParams)
            -> Result<InitializeResult, McpError>,
    ],
}

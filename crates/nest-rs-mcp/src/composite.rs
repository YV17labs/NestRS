//! [`CompositeHandler`] — the one `ServerHandler` several `#[mcp]` hosts share.
//!
//! Every policy degenerates to the lone host's own answer, so one host on a path
//! is served verbatim.
//!
//! | Kind | Methods | Behaviour |
//! |---|---|---|
//! | **Listing** | `tools/list`, `prompts/list`, `resources/list`, `resources/templates/list` | Every host is asked; the entries are concatenated in registration order onto the first host's envelope. |
//! | **Addressed** | `tools/call`, `prompts/get`, `resources/read`, `tasks/*`, `resources/subscribe`, custom methods | Routed to the host that owns the name; failing that, offered to each host in turn until one does not answer *not-found*. |
//! | **Broadcast** | `logging/setLevel`, every notification | Delivered to every host. |
//! | **Declaration** | `initialize`, `discover`, `get_info`, `supported_protocol_versions` | The resolved [`ResolvedIdentity`] states the identity; capabilities are unioned, protocol versions intersected, instructions declared-or-joined. |
//!
//! Pagination cursors are not merged across hosts: a host returning one on a
//! shared path is reported at `warn`.

#![expect(
    deprecated,
    reason = "rmcp still routes the deprecated methods for legacy protocol versions"
)]

use std::borrow::Cow;
use std::sync::Arc;

use nest_rs_core::Container;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CancelTaskParams, CancelledNotificationParam,
    CompleteRequestParams, CompleteResult, CustomNotification, CustomRequest, CustomResult,
    DiscoverResult, ErrorCode, GetPromptRequestParams, GetPromptResponse, GetTaskParams,
    GetTaskResult, InitializeRequestParams, InitializeResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ServerCapabilities, ServerConfig, SetLevelRequestParams, SubscribeRequestParams,
    SubscriptionFilter, Tool, UnsubscribeRequestParams, UpdateTaskParams,
};
use rmcp::service::{NotificationContext, RequestContext, RoleServer, SubscriptionContext};

use crate::McpError;
use crate::guard::BoxFuture;
use crate::host::McpHost;
use crate::identity::ResolvedIdentity;
use crate::registry::{McpHostMeta, ToolIndex};

/// One host as the mount resolved it.
struct MountedHost {
    name: &'static str,
    host: Arc<dyn McpHost>,
}

/// The merged handler for one MCP endpoint, built per session.
pub struct CompositeHandler {
    /// The path's hosts, in registration order.
    hosts: Vec<MountedHost>,
    /// Tool name → position in [`hosts`](Self::hosts), built once at mount.
    tools: Arc<ToolIndex>,
    /// What this endpoint was declared to be, app and host merged.
    identity: Arc<ResolvedIdentity>,
    path: Arc<str>,
}

impl CompositeHandler {
    pub(crate) fn build(
        container: &Container,
        path: Arc<str>,
        hosts: &[Arc<McpHostMeta>],
        tools: Arc<ToolIndex>,
        identity: Arc<ResolvedIdentity>,
    ) -> Self {
        Self {
            hosts: hosts
                .iter()
                .map(|meta| MountedHost {
                    name: meta.host(),
                    host: meta.build(container),
                })
                .collect(),
            tools,
            identity,
            path,
        }
    }

    /// The endpoint's own declaration laid over a host's `initialize` result; the
    /// protocol version stays the host's, which it negotiated.
    fn overlay_own_declaration(&self, mut result: InitializeResult) -> InitializeResult {
        if self.declares_itself() {
            let merged = ServerHandler::get_info(self);
            result.capabilities = merged.capabilities;
            result.instructions = merged.instructions;
            result.server_info = merged.server_info;
        }
        result
    }

    /// Whether the endpoint, not its lone host, answers a declaration question:
    /// a lone undeclared host's own `initialize` / `discover` override is the answer.
    fn declares_itself(&self) -> bool {
        self.identity.is_declared() || self.hosts.len() > 1
    }

    /// The endpoint's first host, whose declaration stands in for the endpoint's.
    fn primary(&self) -> Option<&MountedHost> {
        self.hosts.first()
    }

    /// The lone host on this path: a fast path sparing a `RequestContext` clone per
    /// host, answering what the merge would, `discover` excepted.
    fn single(&self) -> Option<&MountedHost> {
        match self.hosts.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The host that declares `name`, through the index built at mount: rmcp's
    /// `#[tool_handler]` rebuilds the whole `ToolRouter` on each `get_tool`.
    fn owner_of_tool(&self, name: &str) -> Option<&MountedHost> {
        self.tools
            .get(name)
            .and_then(|position| self.hosts.get(*position))
    }

    /// Offer an addressed operation to each host in turn, returning the first
    /// answer that is not *not-found*, else the last not-found.
    async fn route<'a, T, F>(&'a self, call: F) -> Result<T, McpError>
    where
        F: Fn(&'a dyn McpHost) -> BoxFuture<'a, Result<T, McpError>>,
    {
        let mut unhandled = None;
        for host in &self.hosts {
            match call(host.host.as_ref()).await {
                Ok(value) => return Ok(value),
                Err(err) if is_not_found(&err) => unhandled = Some(err),
                Err(err) => return Err(err),
            }
        }
        Err(unhandled.unwrap_or_else(no_hosts))
    }

    fn warn_cursor(&self, host: &MountedHost, method: &str) {
        tracing::warn!(
            target: crate::TARGET,
            path = &*self.path,
            host = host.name,
            method,
            reason = "pagination_across_hosts",
            "an MCP host on a shared path returned a pagination cursor — the merged page drops it",
        );
    }
}

/// Unreachable through `#[mcp]`, where a path exists because a host claimed it.
fn no_hosts() -> McpError {
    McpError::internal_error("no MCP host serves this endpoint".to_owned(), None)
}

/// Whether an error means *this host does not serve that name*, which lets the
/// merge try the next host; rmcp's `ToolRouter` misses with this exact `INVALID_PARAMS`.
fn is_not_found(err: &McpError) -> bool {
    err.code == ErrorCode::METHOD_NOT_FOUND
        || err.code == ErrorCode::RESOURCE_NOT_FOUND
        || (err.code == ErrorCode::INVALID_PARAMS && err.message == "tool not found")
}

/// `a || b`, over two optional flags that are each *absent* rather than false.
fn or_flag(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a || b),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

fn union<T>(into: &mut Option<T>, from: Option<T>)
where
    T: Default + IntoIterator + Extend<<T as IntoIterator>::Item>,
{
    if let Some(from) = from {
        into.get_or_insert_with(T::default).extend(from);
    }
}

fn merge_capabilities(into: &mut ServerCapabilities, from: ServerCapabilities) {
    union(&mut into.experimental, from.experimental);
    union(&mut into.extensions, from.extensions);
    union(&mut into.logging, from.logging);
    union(&mut into.completions, from.completions);
    if let Some(prompts) = from.prompts {
        let slot = into.prompts.get_or_insert_with(Default::default);
        slot.list_changed = or_flag(slot.list_changed, prompts.list_changed);
    }
    if let Some(resources) = from.resources {
        let slot = into.resources.get_or_insert_with(Default::default);
        slot.subscribe = or_flag(slot.subscribe, resources.subscribe);
        slot.list_changed = or_flag(slot.list_changed, resources.list_changed);
    }
    if let Some(tools) = from.tools {
        let slot = into.tools.get_or_insert_with(Default::default);
        slot.list_changed = or_flag(slot.list_changed, tools.list_changed);
    }
}

/// The protocol versions **every** list carries, in the first list's order; the
/// boot check and the wire answer share it.
pub(crate) fn common_protocol_versions(
    lists: &[Cow<'static, [ProtocolVersion]>],
) -> Vec<ProtocolVersion> {
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    first
        .iter()
        .filter(|version| rest.iter().all(|list| list.contains(version)))
        .cloned()
        .collect()
}

/// One listing method: ask every host, concatenate onto the first's envelope.
macro_rules! merged_listing {
    ($name:ident, $out:ty, $items:ident, $method:literal) => {
        async fn $name(
            &self,
            request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> Result<$out, McpError> {
            let Some((first, rest)) = self.hosts.split_first() else {
                return Err(no_hosts());
            };
            let mut merged = first.host.$name(request.clone(), context.clone()).await?;
            if rest.is_empty() {
                // A lone host's cursor is still followable.
                return Ok(merged);
            }
            if merged.next_cursor.is_some() {
                self.warn_cursor(first, $method);
            }
            for host in rest {
                let more = host.host.$name(request.clone(), context.clone()).await?;
                if more.next_cursor.is_some() {
                    self.warn_cursor(host, $method);
                }
                merged.$items.extend(more.$items);
            }
            merged.next_cursor = None;
            Ok(merged)
        }
    };
}

/// One addressed method: routed through [`CompositeHandler::route`].
macro_rules! addressed {
    ($name:ident ( $req:ty ) -> $out:ty) => {
        async fn $name(
            &self,
            request: $req,
            context: RequestContext<RoleServer>,
        ) -> Result<$out, McpError> {
            if let Some(only) = self.single() {
                return only.host.$name(request, context).await;
            }
            self.route(|host| host.$name(request.clone(), context.clone()))
                .await
        }
    };
}

/// One notification: delivered to every host.
macro_rules! broadcast_notification {
    ($name:ident ( $($arg:ident : $ty:ty),* )) => {
        async fn $name(&self, $($arg: $ty,)* context: NotificationContext<RoleServer>) {
            if let Some(only) = self.single() {
                return only.host.$name($($arg,)* context).await;
            }
            for host in &self.hosts {
                host.host.$name($($arg.clone(),)* context.clone()).await;
            }
        }
    };
}

#[deny(clippy::missing_trait_methods)]
impl ServerHandler for CompositeHandler {
    async fn ping(&self, context: RequestContext<RoleServer>) -> Result<(), McpError> {
        if let Some(only) = self.single() {
            return only.host.ping(context).await;
        }
        for host in &self.hosts {
            host.host.ping(context.clone()).await?;
        }
        Ok(())
    }

    /// The first host runs rmcp's own `initialize`, whose negotiation and
    /// `set_peer_info` are crate-private to rmcp; the endpoint's declaration goes on top.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        let Some(first) = self.primary() else {
            return Err(no_hosts());
        };
        let result = first.host.initialize(request, context).await?;
        Ok(self.overlay_own_declaration(result))
    }

    /// rmcp calls this only from the default `initialize`, overridden here; its own
    /// default would ignore the primary host. Not `initialize`, which sets the peer info.
    fn negotiate_initialize(
        &self,
        request: &InitializeRequestParams,
    ) -> Result<InitializeResult, McpError> {
        let Some(first) = self.primary() else {
            return Err(no_hosts());
        };
        Ok(self.overlay_own_declaration(first.host.negotiate_initialize(request)?))
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, McpError> {
        if !self.declares_itself() {
            // Whether a lone host overrides `discover` cannot be asked; the
            // rebuild below would discard its override.
            if let Some(only) = self.single() {
                return only.host.discover(context).await;
            }
        }
        Ok(DiscoverResult::from_server_info(
            ServerHandler::supported_protocol_versions(self).into_owned(),
            ServerHandler::get_info(self),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if let Some(only) = self.single() {
            return only.host.call_tool(request, context).await;
        }
        if let Some(owner) = self.owner_of_tool(&request.name) {
            return owner.host.call_tool(request, context).await;
        }
        // A host whose router the mount could not read still gets its turn.
        self.route(|host| host.call_tool(request.clone(), context.clone()))
            .await
    }

    merged_listing!(list_tools, ListToolsResult, tools, "tools/list");

    addressed!(get_prompt(GetPromptRequestParams) -> GetPromptResponse);
    merged_listing!(list_prompts, ListPromptsResult, prompts, "prompts/list");

    addressed!(read_resource(ReadResourceRequestParams) -> ReadResourceResponse);
    merged_listing!(
        list_resources,
        ListResourcesResult,
        resources,
        "resources/list"
    );
    merged_listing!(
        list_resource_templates,
        ListResourceTemplatesResult,
        resource_templates,
        "resources/templates/list"
    );
    addressed!(subscribe(SubscribeRequestParams) -> ());
    addressed!(unsubscribe(UnsubscribeRequestParams) -> ());

    /// A host with nothing to complete answers an empty `Ok`, not *not-found*: the
    /// first non-empty completion wins, an empty one or a refusal is the fallback.
    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        let mut fallback: Option<Result<CompleteResult, McpError>> = None;
        for host in &self.hosts {
            match host.host.complete(request.clone(), context.clone()).await {
                Ok(result) if !result.completion.values.is_empty() => return Ok(result),
                Ok(result) => {
                    fallback.get_or_insert(Ok(result));
                }
                Err(err) if is_not_found(&err) => {
                    fallback.get_or_insert(Err(err));
                }
                Err(err) => return Err(err),
            }
        }
        fallback.unwrap_or_else(|| Err(no_hosts()))
    }

    /// Every host is told; accepted by any is accepted, but a real failure is
    /// reported even when a peer accepted.
    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        let mut accepted = false;
        let mut failure = None;
        let mut refusal = None;
        for host in &self.hosts {
            match host.host.set_level(request.clone(), context.clone()).await {
                Ok(()) => accepted = true,
                Err(err) if is_not_found(&err) => {
                    refusal.get_or_insert(err);
                }
                Err(err) => {
                    failure.get_or_insert(err);
                }
            }
        }
        match (failure, accepted) {
            (Some(err), _) => Err(err),
            (None, true) => Ok(()),
            (None, false) => Err(refusal.unwrap_or_else(no_hosts)),
        }
    }

    addressed!(get_task(GetTaskParams) -> GetTaskResult);
    addressed!(update_task(UpdateTaskParams) -> ());
    addressed!(cancel_task(CancelTaskParams) -> ());

    addressed!(on_custom_request(CustomRequest) -> CustomResult);

    broadcast_notification!(on_cancelled(notification: CancelledNotificationParam));
    broadcast_notification!(on_progress(notification: ProgressNotificationParam));
    broadcast_notification!(on_initialized());
    broadcast_notification!(on_roots_list_changed());
    broadcast_notification!(on_custom_notification(notification: CustomNotification));

    /// Served by the host that accepted the client's filter, else the primary
    /// host, whose own `listen` answers (rmcp's default: hold until cancelled).
    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let requested = context.requested().clone();
        let owner = self
            .hosts
            .iter()
            .find(|host| host.host.accepted_subscription_filter(&requested).is_some())
            .or_else(|| self.primary());
        match owner {
            Some(host) => host.host.listen(context).await,
            None => Err(no_hosts()),
        }
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.hosts
            .iter()
            .find_map(|host| host.host.accepted_subscription_filter(requested))
    }

    /// Routed through the index first, so it rebuilds one host's `ToolRouter`.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        match self.owner_of_tool(name) {
            Some(owner) => owner.host.get_tool(name),
            None => self.hosts.iter().find_map(|host| host.host.get_tool(name)),
        }
    }

    /// What **every** host on the path implements; an empty intersection is
    /// refused at boot, so the primary's fallback is unreachable.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        let Some(primary) = self.primary() else {
            return Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS);
        };
        let primary = primary.host.supported_protocol_versions();
        if self.hosts.len() == 1 {
            return primary;
        }
        let declared: Vec<Cow<'static, [ProtocolVersion]>> = self
            .hosts
            .iter()
            .map(|host| host.host.supported_protocol_versions())
            .collect();
        match common_protocol_versions(&declared) {
            common if common.is_empty() => primary,
            common => Cow::Owned(common),
        }
    }

    /// A declaration replaces only `serverInfo` and `instructions`; capabilities
    /// stay the union of what the hosts serve. Undeclared, the first host's identity stands.
    fn get_info(&self) -> ServerConfig {
        let mut infos = self.hosts.iter().map(|host| host.host.get_info());
        let Some(mut merged) = infos.next() else {
            return ServerConfig::new(ServerCapabilities::default());
        };
        let mut instructions: Vec<String> = merged.instructions.take().into_iter().collect();
        for info in infos {
            merge_capabilities(&mut merged.capabilities, info.capabilities);
            instructions.extend(info.instructions);
        }
        merged.instructions = (!instructions.is_empty()).then(|| instructions.join("\n\n"));

        if let Some(info) = self.identity.implementation() {
            merged.server_info = info.clone();
        }
        if let Some(declared) = self.identity.instructions() {
            merged.instructions = Some(declared.to_owned());
        }
        merged
    }
}

#[cfg(test)]
mod protocol_intersection {
    use super::*;

    fn list(versions: &[ProtocolVersion]) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(versions.to_vec())
    }

    /// Widening is the silent failure: the boot check would pass, and a client
    /// would find out at a request.
    #[test]
    fn is_what_every_host_declares_and_never_more() {
        let older = ProtocolVersion::V_2024_11_05;
        let shared = ProtocolVersion::V_2025_06_18;
        let newer = ProtocolVersion::V_2025_11_25;

        assert_eq!(
            common_protocol_versions(&[
                list(&[older.clone(), shared.clone()]),
                list(&[shared.clone(), newer.clone()]),
            ]),
            vec![shared.clone()],
            "a version only one host declares is not the endpoint's to offer",
        );
        assert_eq!(
            common_protocol_versions(&[
                list(std::slice::from_ref(&older)),
                list(std::slice::from_ref(&newer)),
            ]),
            Vec::new(),
            "no overlap is empty, which is what the boot refuses rather than \
             quietly narrowing to nothing",
        );
        assert_eq!(
            common_protocol_versions(&[list(&[older.clone(), shared.clone()])]),
            vec![older, shared],
            "one host alone keeps its whole list — the merge is what narrows",
        );
    }
}

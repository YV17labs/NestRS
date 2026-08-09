//! [`CompositeHandler`] — the one `ServerHandler` several `#[mcp]` hosts share.
//!
//! # The rule
//!
//! *One host on a path is served verbatim*, so adding the merge changed nothing
//! for the shape every existing app has. That is a property of the policies
//! below, not a second code path bolted on: each one **degenerates** to the lone
//! host's own answer at N=1. [`single`](CompositeHandler::single) appears only
//! where the merged path would otherwise pay for aggregation it cannot use —
//! chiefly a deep `RequestContext` clone per host — plus `discover`, the one
//! place it is genuinely a policy and says so.
//!
//! # The policy, per kind of operation
//!
//! | Kind | Methods | Behaviour |
//! |---|---|---|
//! | **Listing** | `tools/list`, `prompts/list`, `resources/list`, `resources/templates/list` | Every host is asked; the entries are concatenated in registration order onto the first host's envelope. |
//! | **Addressed** | `tools/call`, `prompts/get`, `resources/read`, `tasks/*`, `resources/subscribe`, custom methods | Routed to the host that owns the name; failing that, offered to each host in turn until one does not answer *not-found*. |
//! | **Broadcast** | `logging/setLevel`, every notification | Delivered to every host. |
//! | **Declaration** | `initialize`, `discover`, `get_info`, `supported_protocol_versions` | The declared [`McpEndpoint`] states the identity; capabilities are unioned, protocol versions intersected, instructions declared-or-joined. |
//!
//! Identity is the one part a *host* cannot answer for, because an endpoint
//! reports one `serverInfo` however many features share it — so the app
//! declares it (`McpOptions::endpoints`), and undeclared it falls back to the
//! first host's with a boot `warn`. See [`McpEndpoint`].
//!
//! `tools/call` is routed through an index the mount builds once, name → host,
//! from what each host's `#[tool_router]` declares — deliberately *not* by
//! asking each host's `get_tool`, which rmcp answers by rebuilding that host's
//! entire router. A name the index does not carry falls through to the
//! offer-each-host path, which is what serves a host whose router the mount
//! could not read. Two hosts claiming one name never gets this far: it is a boot
//! error (`registry::check_duplicate_tools`), because within an endpoint MCP
//! addresses a tool by bare name and the loser would simply be unreachable.
//!
//! # What the merge does not do
//!
//! Cursor-based pagination across hosts. Each host's own cursor is meaningless
//! to its peers, so a host that returns one on an aggregated path is reported
//! at `warn` rather than silently truncating the merged list. rmcp's routers
//! never paginate, so this is a guard rail, not a live limitation.

// `subscribe` / `unsubscribe` are SEP-2575-deprecated in rmcp but still routed
// for legacy protocol versions.
#![expect(deprecated)]

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
    ServerCapabilities, ServerInfo, SetLevelRequestParams, SubscribeRequestParams,
    SubscriptionFilter, Tool, UnsubscribeRequestParams, UpdateTaskParams,
};
use rmcp::service::{NotificationContext, RequestContext, RoleServer, SubscriptionContext};

use crate::McpError;
use crate::guard::BoxFuture;
use crate::host::McpHost;
use crate::identity::McpEndpoint;
use crate::registry::{McpHostMeta, ToolIndex};

/// One host as the mount resolved it: the name a diagnostic prints, and the
/// live instance for this session.
struct MountedHost {
    name: &'static str,
    host: Arc<dyn McpHost>,
}

/// The merged handler for one MCP endpoint. Built per session, like the single
/// host it replaced — see [`crate::endpoint`].
pub struct CompositeHandler {
    /// The path's hosts, in registration order. Never empty in practice: the
    /// mount exists because a host claimed the path.
    hosts: Vec<MountedHost>,
    /// Tool name → position in [`hosts`](Self::hosts), built once at mount.
    tools: Arc<ToolIndex>,
    /// What the app declared this endpoint to be, when it declared anything.
    identity: Option<Arc<McpEndpoint>>,
    path: &'static str,
}

impl CompositeHandler {
    /// Build one instance of every host contributing to a path.
    pub(crate) fn build(
        container: &Container,
        path: &'static str,
        hosts: &[Arc<McpHostMeta>],
        tools: Arc<ToolIndex>,
        identity: Option<Arc<McpEndpoint>>,
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

    /// Whether the *endpoint* answers a declaration question, rather than the
    /// host that happens to serve it.
    ///
    /// True once the app declared an [`McpEndpoint`] — it said what this
    /// endpoint is — or once several hosts share the path, where no single one
    /// of them can speak for it. False for the lone undeclared host, which *is*
    /// the endpoint: its own `initialize` / `discover` override is the answer,
    /// and rebuilding one from `get_info` would silently discard it.
    fn declares_itself(&self) -> bool {
        self.identity.is_some() || self.hosts.len() > 1
    }

    /// The endpoint's first host — whose own declaration stands in for the
    /// endpoint's (its identity, its handshake).
    ///
    /// The `Option` is structural, not a real case, and it is answered **here**
    /// rather than re-invented by every method that needs a host to speak for
    /// the path.
    fn primary(&self) -> Option<&MountedHost> {
        self.hosts.first()
    }

    /// The lone host on this path, when there is exactly one.
    ///
    /// Used only where the merged path would otherwise pay for aggregation it
    /// cannot use — chiefly a deep `RequestContext` clone per host, on the shape
    /// every single-host app has. It is a fast path, not a second policy: every
    /// site below that takes it produces exactly what the merged path would,
    /// with `discover` the one documented exception.
    fn single(&self) -> Option<&MountedHost> {
        match self.hosts.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The host that declares `name`, through the index built at mount.
    ///
    /// Deliberately not a `get_tool` scan: rmcp's `#[tool_handler]` rebuilds the
    /// host's whole `ToolRouter` — every tool, every schema — on each `get_tool`
    /// call, so scanning would re-assemble every router on every `tools/call`.
    /// A name the index does not carry falls through to
    /// [`route`](Self::route), which is what serves a host whose router the
    /// index could not read.
    fn owner_of_tool(&self, name: &str) -> Option<&MountedHost> {
        self.tools
            .get(name)
            .and_then(|position| self.hosts.get(*position))
    }

    /// Offer an addressed operation to each host in turn, returning the first
    /// answer that is not *not-found*. The last not-found is what surfaces when
    /// nobody owns it, so the caller sees a real MCP error rather than a
    /// framework one.
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

    /// A host returned a pagination cursor on a path it shares. Its peers'
    /// entries are already in the merged page, so the cursor cannot be followed
    /// — say so rather than hand back a list that looks complete.
    fn warn_cursor(&self, host: &MountedHost, method: &str) {
        tracing::warn!(
            target: "nest_rs::mcp",
            path = self.path,
            host = host.name,
            method,
            reason = "pagination_across_hosts",
            "an MCP host on a shared path returned a pagination cursor — the merged page drops it",
        );
    }
}

/// The error a client gets when a mount somehow has no hosts. Unreachable
/// through `#[mcp]` (a path exists because a host claimed it), but the merge is
/// written over a slice and a slice can be empty.
fn no_hosts() -> McpError {
    McpError::internal_error("no MCP host serves this endpoint".to_owned(), None)
}

/// Whether an error means *this host does not serve that name*, as opposed to
/// *serving it failed*. Only the first kind lets the merge try the next host.
///
/// Three shapes, all from rmcp: the trait default's `method_not_found`, a
/// resource host's `resource_not_found`, and `ToolRouter`'s own miss — which is
/// an `INVALID_PARAMS` carrying this exact message.
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

/// Fold one optional collection into another, creating it if absent.
fn union<T>(into: &mut Option<T>, from: Option<T>)
where
    T: Default + IntoIterator + Extend<<T as IntoIterator>::Item>,
{
    if let Some(from) = from {
        into.get_or_insert_with(T::default).extend(from);
    }
}

/// Union of two capability declarations: a capability any host serves is a
/// capability the endpoint serves.
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

/// The protocol versions **every** list carries, in the first list's order.
///
/// One endpoint negotiates one version, so a merged mount may only advertise
/// what all its hosts implement. Shared with `registry`'s boot check, so the
/// verdict at boot and the answer on the wire are the same computation.
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
                // A lone host's cursor is its own and still followable — the
                // merge is what makes one meaningless, so only aggregation
                // drops it.
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

impl ServerHandler for CompositeHandler {
    // --- lifecycle & discovery ---------------------------------------------

    async fn ping(&self, context: RequestContext<RoleServer>) -> Result<(), McpError> {
        if let Some(only) = self.single() {
            return only.host.ping(context).await;
        }
        for host in &self.hosts {
            host.host.ping(context.clone()).await?;
        }
        Ok(())
    }

    /// The first host runs rmcp's own `initialize` — protocol negotiation and
    /// `set_peer_info` live in a `pub(crate)` helper no handler can call, so the
    /// merge borrows them rather than reimplementing them — and the parts that
    /// are the *endpoint's* are then replaced by the endpoint's own answer.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        let Some(first) = self.primary() else {
            return Err(no_hosts());
        };
        let mut result = first.host.initialize(request, context).await?;
        if self.declares_itself() {
            let merged = ServerHandler::get_info(self);
            result.capabilities = merged.capabilities;
            result.instructions = merged.instructions;
            result.server_info = merged.server_info;
        }
        Ok(result)
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, McpError> {
        if !self.declares_itself() {
            // A lone undeclared host may override `discover`, and the branch
            // below cannot ask whether it did — it can only rebuild the default
            // from the merged declaration. Once the endpoint declares itself,
            // that declaration is the answer, override or not.
            if let Some(only) = self.single() {
                return only.host.discover(context).await;
            }
        }
        Ok(DiscoverResult::from_server_info(
            ServerHandler::supported_protocol_versions(self).into_owned(),
            ServerHandler::get_info(self),
        ))
    }

    // --- tools ---------------------------------------------------------------

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
        // The index does not carry the name — a host whose router the mount
        // could not read still gets its turn.
        self.route(|host| host.call_tool(request.clone(), context.clone()))
            .await
    }

    merged_listing!(list_tools, ListToolsResult, tools, "tools/list");

    // --- prompts --------------------------------------------------------------

    addressed!(get_prompt(GetPromptRequestParams) -> GetPromptResponse);
    merged_listing!(list_prompts, ListPromptsResult, prompts, "prompts/list");

    // --- resources ------------------------------------------------------------

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

    // --- completion & logging --------------------------------------------------

    /// A host with nothing to complete answers `Ok` with an empty list rather
    /// than *not-found*, so [`route`](CompositeHandler::route) cannot tell it
    /// apart from a real answer: the first **non-empty** completion wins, and an
    /// empty one is kept only as the fallback.
    ///
    /// A host that refuses outright is remembered too, so one host alone on a
    /// path still surfaces its own refusal instead of an invented empty result.
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

    /// A logging level is a property of the *connection*, not of one host, so
    /// every host is told. Accepted by any ⇒ accepted; a real failure is
    /// reported even when a peer accepted, because a half-applied level is a
    /// worse answer than a refusal.
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
                // Kept, not discarded: with nobody accepting, the client should
                // read a host's own refusal rather than one this merge invented.
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

    // --- tasks (SEP-2663) --------------------------------------------------------

    addressed!(get_task(GetTaskParams) -> GetTaskResult);
    addressed!(update_task(UpdateTaskParams) -> ());
    addressed!(cancel_task(CancelTaskParams) -> ());

    // --- custom methods ------------------------------------------------------------

    addressed!(on_custom_request(CustomRequest) -> CustomResult);

    // --- notifications --------------------------------------------------------------

    broadcast_notification!(on_cancelled(notification: CancelledNotificationParam));
    broadcast_notification!(on_progress(notification: ProgressNotificationParam));
    broadcast_notification!(on_initialized());
    broadcast_notification!(on_roots_list_changed());
    broadcast_notification!(on_custom_notification(notification: CustomNotification));

    /// The subscription belongs to whichever host accepted the client's filter
    /// — the same host [`accepted_subscription_filter`](Self::accepted_subscription_filter)
    /// answered for. With none, it falls to the primary host, whose own `listen`
    /// (rmcp's default: hold until cancelled) is the right answer and keeps a
    /// lone host's override from being bypassed.
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

    // --- synchronous accessors ---------------------------------------------------------

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.hosts
            .iter()
            .find_map(|host| host.host.accepted_subscription_filter(requested))
    }

    /// Routed through the index first, so the common case rebuilds **one**
    /// host's `ToolRouter` rather than every host's.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        match self.owner_of_tool(name) {
            Some(owner) => owner.host.get_tool(name),
            None => self.hosts.iter().find_map(|host| host.host.get_tool(name)),
        }
    }

    /// One endpoint negotiates one version, so it may only advertise what
    /// **every** host on the path implements — [`common_protocol_versions`],
    /// the same computation the boot check verdicts on. An empty intersection is
    /// refused at boot, so falling back to the primary host's list is a guard
    /// rail rather than a live case.
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

    /// What the endpoint says it is.
    ///
    /// **Identity is declared, capabilities are observed.** A declared
    /// [`McpEndpoint`] replaces exactly what it states — `serverInfo` always,
    /// `instructions` when it wrote any — because those are the app's word about
    /// its own server. It can never add a capability: those stay the union of
    /// what the hosts actually serve, so the endpoint cannot advertise a surface
    /// nobody implements.
    ///
    /// Undeclared, the endpoint borrows its first host's identity and joins the
    /// hosts' instructions rather than dropping all but one. That is a fallback,
    /// and a shared path taking it is reported at boot
    /// (`registry::warn_undeclared_identity`) — at N=1 it is not a fallback at
    /// all: one host alone *is* the server, which is the shape every MCP SDK
    /// builds.
    fn get_info(&self) -> ServerInfo {
        let mut infos = self.hosts.iter().map(|host| host.host.get_info());
        let Some(mut merged) = infos.next() else {
            return ServerInfo::new(ServerCapabilities::default());
        };
        let mut instructions: Vec<String> = merged.instructions.take().into_iter().collect();
        for info in infos {
            merge_capabilities(&mut merged.capabilities, info.capabilities);
            instructions.extend(info.instructions);
        }
        merged.instructions = (!instructions.is_empty()).then(|| instructions.join("\n\n"));

        if let Some(identity) = &self.identity {
            merged.server_info = identity.implementation().clone();
            if let Some(declared) = identity.declared_instructions() {
                merged.instructions = Some(declared.to_owned());
            }
        }
        merged
    }
}

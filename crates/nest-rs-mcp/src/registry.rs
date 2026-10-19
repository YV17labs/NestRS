//! The per-path host registry: [`register_host`] records an [`McpHostMeta`] per
//! `#[mcp]` host and, for the **first** host on a path, attaches the one
//! [`HttpEndpointMeta`] that mounts them all, merged into a [`CompositeHandler`].
//!
//! Metadata, not `inventory`: it is attached from `Discoverable::register`, so a
//! host whose module the app does not import contributes nothing. The chains,
//! [`McpConfig`](crate::McpConfig) and the session store resolve once per path,
//! when its [`McpMount`] is built from the container; every endpoint's checks
//! and every operation's site run at the kernel's wiring step, in every app
//! that imports a host, whatever it serves.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use nest_rs_core::__private::Site;
use nest_rs_core::{
    Container, ContainerBuilder, Discovery, Handler, Posture, WiringContribution, anyhow,
};
use nest_rs_http::__private::{OriginCheck, OriginRefused, checks_origin};
use nest_rs_http::{DetachedWork, HttpEndpointMeta, HttpRoutes, normalize_mount_path};
use rmcp::model::{Prompt, ProtocolVersion, ServerCapabilities, ServerConfig, Tool};

use crate::composite::{CompositeHandler, common_protocol_versions};
use crate::endpoint::{McpMount, endpoint};
use crate::host::McpHost;
use crate::identity::{McpIdentity, ResolvedIdentity};
use crate::module::McpModule;
use crate::operation::McpOperationKind;
use crate::view::McpView;

/// The `HttpEndpointMeta` label every MCP mount carries.
pub(crate) const MCP_LABEL: &str = "mcp";

/// Where a bare `#[mcp]` mounts; a host that wants its own endpoint writes the
/// whole path instead.
pub const DEFAULT_PATH: &str = "/mcp";

/// The site an MCP endpoint's request at `path` runs as before its operations —
/// `path` empty for the default one: a gated endpoint, so an authentication
/// guard refuses an anonymous call. `#[mcp]` declares one per host, the first
/// host on a path the one its requests run as.
pub const fn declare_endpoint(path: &'static str) -> Handler {
    nest_rs_http::declare_endpoint::<McpModule>("McpModule", mount_address(path), Posture::Gated)
}

/// The address an endpoint declared at `path` is mounted at: the default for
/// none, and without the trailing slash a declaration may carry (`/mcp/`
/// mounts `/mcp`).
const fn mount_address(path: &'static str) -> &'static str {
    match path.as_bytes() {
        [] => DEFAULT_PATH,
        [_, .., b'/'] => path.split_at(path.len() - 1).0,
        _ => path,
    }
}

/// The site of the endpoint at [`DEFAULT_PATH`], and of one an app mounts by
/// hand through [`endpoint`].
pub static ENDPOINT: Handler = declare_endpoint(DEFAULT_PATH);

/// Build one host instance from the live container, per MCP session.
type BuildHost = fn(&Container) -> Arc<dyn McpHost>;

/// What a host declares *statically* — its routers, evaluated without an
/// instance, for the mount's index and the boot's duplicate checks.
type StaticRoutes = fn() -> DeclaredRoutes;

/// The sites a host's `#[tools]` declares, one per operation, which the boot
/// composes.
type OperationSites = fn() -> Vec<&'static Site>;

/// The registry the boot's wiring step checks every endpoint and composes
/// every operation's site for.
const ENDPOINTS: &str = "nest_rs::mcp::endpoints";

/// The tools and prompts a host's routers declare, read without an instance.
pub struct DeclaredRoutes {
    tools: Vec<Tool>,
    prompts: Vec<Prompt>,
    complete: bool,
}

impl DeclaredRoutes {
    /// `tools` and `prompts` as the host's routers list them; `complete` when
    /// they are every tool and prompt the host serves, as `#[tools]` writes
    /// its whole handler.
    pub fn new(tools: Vec<Tool>, prompts: Vec<Prompt>, complete: bool) -> Self {
        Self {
            tools,
            prompts,
            complete,
        }
    }

    fn tool_names(&self) -> impl Iterator<Item = String> + '_ {
        self.tools.iter().map(|tool| tool.name.to_string())
    }

    fn prompt_names(&self) -> impl Iterator<Item = String> + '_ {
        self.prompts.iter().map(|prompt| prompt.name.clone())
    }
}

/// One `#[mcp]` host's contribution to the endpoint at its path.
pub struct McpHostMeta {
    path: String,
    host: &'static str,
    /// The host's type path, as an operation's view names it.
    type_name: &'static str,
    identity: McpIdentity,
    build: BuildHost,
    routes: StaticRoutes,
    sites: OperationSites,
}

impl McpHostMeta {
    /// The endpoint path this host contributes to — the URL a client calls.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// What this host declares about the endpoint it serves.
    pub fn identity(&self) -> &McpIdentity {
        &self.identity
    }

    /// The host struct's name (`PostsTool`).
    pub fn host(&self) -> &'static str {
        self.host
    }

    /// The tools this host declares through rmcp's `#[tool_router]`; empty for
    /// a host that hand-writes `list_tools`/`call_tool`.
    pub fn declared_tools(&self) -> Vec<Tool> {
        (self.routes)().tools
    }

    pub(crate) fn declared_routes(&self) -> DeclaredRoutes {
        (self.routes)()
    }

    /// The sites its operations run, one per tool and prompt `#[tools]`
    /// declares.
    pub(crate) fn operation_sites(&self) -> Vec<&'static Site> {
        (self.sites)()
    }

    pub(crate) fn build(&self, container: &Container) -> Arc<dyn McpHost> {
        (self.build)(container)
    }
}

/// Empty fallback so `#[mcp]` can ask **any** host for its declared tools: an
/// inherent `tool_router()` wins over this trait's.
pub trait DefaultToolRouter: Sized + Send + Sync + 'static {
    /// No router of its own.
    fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        rmcp::handler::server::router::tool::ToolRouter::default()
    }
}

impl<T: Send + Sync + 'static> DefaultToolRouter for T {}

/// Empty fallback so `#[mcp]` can ask **any** host for its declared prompts:
/// an inherent `prompt_router()` wins over this trait's.
pub trait DefaultPromptRouter: Sized + Send + Sync + 'static {
    /// No router of its own.
    fn prompt_router() -> rmcp::handler::server::router::prompt::PromptRouter<Self> {
        rmcp::handler::server::router::prompt::PromptRouter::default()
    }
}

impl<T: Send + Sync + 'static> DefaultPromptRouter for T {}

/// Fallback so `#[mcp]` can ask **any** host whether its routers are all it
/// serves: only the `#[tools]` impl's inherent function answers yes.
pub trait DefaultDeclaredRoutes: Sized + 'static {
    /// A host writing rmcp's handler by hand may serve more than its routers.
    fn __nestrs_mcp_declares_every_route() -> bool {
        false
    }
}

impl<T: 'static> DefaultDeclaredRoutes for T {}

/// Empty fallback so the struct-level `#[mcp]` can ask **any** host which layers
/// its operations declared; the `#[tools]` impl's inherent function wins. One
/// tuple, so keys and names stay index-aligned.
pub trait DefaultOperationLayers: Sized + 'static {
    /// No decorated operations, so no per-operation layers.
    fn __nestrs_mcp_operation_layers() -> (Vec<std::any::TypeId>, Vec<&'static str>) {
        (Vec::new(), Vec::new())
    }

    /// No decorated operations, so no provider a parameter resolves.
    fn __nestrs_mcp_injected_per_request() -> Vec<(std::any::TypeId, &'static str)> {
        Vec::new()
    }

    /// No decorated operations, so no site to compose.
    fn __nestrs_mcp_operation_sites() -> Vec<&'static Site> {
        Vec::new()
    }
}

impl<T: 'static> DefaultOperationLayers for T {}

/// Record `P` as a host of the MCP endpoint at `path`, its operations running
/// the `sites` its `#[tools]` declares, and — for the first host to claim that
/// path — attach the endpoint itself, its requests running as `site`; the
/// first host of the app attaches the wiring that checks every endpoint.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one thing the decorator declared about its host; a struct would only rename them"
)]
pub fn register_host<P: 'static>(
    builder: ContainerBuilder,
    site: &'static Handler,
    path: &'static str,
    host: &'static str,
    identity: McpIdentity,
    build: BuildHost,
    routes: StaticRoutes,
    sites: OperationSites,
) -> ContainerBuilder {
    let path = normalize_mount_path(match path {
        "" => DEFAULT_PATH,
        declared => declared,
    });

    // Exactly one `HttpEndpointMeta` per path, so a mount path stays its owner's
    // exclusive namespace; a peer host only adds its contribution.
    let claimed = builder
        .attached_meta::<HttpEndpointMeta>()
        .any(|meta| meta.label() == MCP_LABEL && meta.path() == path);

    let wired = builder
        .attached_meta::<WiringContribution>()
        .any(|wiring| wiring.name() == ENDPOINTS);

    let builder = builder.attach_meta::<P, McpHostMeta>(McpHostMeta {
        path: path.clone(),
        host,
        type_name: std::any::type_name::<P>(),
        identity,
        build,
        routes,
        sites,
    });
    let builder = if wired {
        builder
    } else {
        builder.provide_wiring(ENDPOINTS, wire_endpoints)
    };

    if claimed {
        return builder;
    }

    let mount_path = path.clone();
    let detached = DetachedWork::new();
    let mount_work = detached.clone();
    builder.attach_meta::<P, HttpEndpointMeta>(checks_origin(
        HttpEndpointMeta::new(site, path, MCP_LABEL, move |container, routes| {
            mount(container, routes, &mount_path, &mount_work);
        })
        .carries::<McpView>()
        .runs_detached(detached)
        .owned_by(host),
        OriginCheck::Every,
        refused_origin,
    ))
}

/// The boot's checks of every endpoint the app's hosts share, and the sites
/// their operations run, composed: a contested identity, a name two hosts
/// claim, protocol versions no host shares, a layer an operation declares that
/// no module provides or two `#[meta]` values of one type refuse the app.
fn wire_endpoints(container: &Container) -> anyhow::Result<()> {
    let metas = Discovery::new(container).meta::<McpHostMeta>();
    let paths: BTreeSet<String> = metas
        .iter()
        .map(|discovered| discovered.meta.path().to_owned())
        .collect();
    for path in &paths {
        check_endpoint(container, path).map_err(|refused| anyhow::anyhow!(refused))?;
    }
    if !metas.is_empty() {
        crate::operation::REQUEST.chains(container)?;
    }
    for discovered in &metas {
        for site in discovered.meta.operation_sites() {
            site.chains(container)?;
        }
    }
    Ok(())
}

/// The checks of the endpoint at `path`, over the hosts that share it.
fn check_endpoint(container: &Container, path: &str) -> Result<(), String> {
    let hosts = resolve(container, path);
    check_identity(container, path, &hosts)?;
    if hosts.len() < 2 {
        return Ok(());
    }
    check_duplicate_tools(path, &hosts)?;
    check_duplicate_prompts(path, &hosts)?;
    warn_undeclared_tools(path, &hosts);
    check_protocol_versions(path, &hosts)
}

/// An MCP request from a browser origin the transport's policy refuses, filed on
/// this edge's target.
fn refused_origin(refused: &OriginRefused) {
    nest_rs_http::__origin_refused!(target: crate::TARGET, refused);
}

/// The identity the endpoint at `path` reports: the declaring host's
/// declaration laid over the app's [`McpIdentity`].
pub fn endpoint_identity(container: &Container, path: &str) -> ResolvedIdentity {
    // The `Err` arm is a boot failure `check_identity` already raised.
    resolve_identity(container, path, &hosts_on(container, path)).unwrap_or_default()
}

/// [`endpoint_identity`] with the failures spelled out, over a host list the
/// caller already holds.
fn resolve_identity(
    container: &Container,
    path: &str,
    hosts: &[Arc<McpHostMeta>],
) -> Result<ResolvedIdentity, String> {
    let declaring: Vec<&Arc<McpHostMeta>> = hosts
        .iter()
        .filter(|meta| !meta.identity().is_empty())
        .collect();

    if declaring.len() > 1 {
        let names: Vec<&str> = declaring.iter().map(|meta| meta.host()).collect();
        return Err(format!(
            "two MCP hosts declare the identity of {path:?}: {} — an endpoint reports one \
             serverInfo, so declare it on one host and let the others inherit it",
            names.join(" and "),
        ));
    }

    let host = declaring.first();
    crate::identity::resolve(
        host.map(|meta| meta.identity()),
        container.get::<McpIdentity>().as_deref(),
    )
    .map_err(|err| err.report(path, host.map_or("<none>", |meta| meta.host())))
}

/// Every host contributing to `path`, in registration order.
pub fn hosts_on(container: &Container, path: &str) -> Vec<Arc<McpHostMeta>> {
    Discovery::new(container)
        .meta::<McpHostMeta>()
        .into_iter()
        .map(|discovered| discovered.meta)
        .filter(|meta| meta.path() == path)
        .collect()
}

/// One host on a path, resolved **once** per boot pass and shared by every
/// check: `build` runs the developer's constructor and `declared_routes`
/// builds a whole router of each kind.
struct ResolvedHost {
    meta: Arc<McpHostMeta>,
    name: &'static str,
    tools: BTreeSet<String>,
    prompts: BTreeSet<String>,
    /// Whether its routers are every tool and prompt it serves.
    complete: bool,
    instance: Arc<dyn McpHost>,
}

impl ResolvedHost {
    /// Whether the boot can read the tools it serves off its router.
    fn reads_tools(&self) -> bool {
        self.complete || !self.tools.is_empty()
    }
}

fn resolve(container: &Container, path: &str) -> Vec<ResolvedHost> {
    hosts_on(container, path)
        .into_iter()
        .map(|meta| {
            let routes = meta.declared_routes();
            ResolvedHost {
                name: meta.host(),
                tools: routes.tool_names().collect(),
                prompts: routes.prompt_names().collect(),
                complete: routes.complete,
                instance: meta.build(container),
                meta,
            }
        })
        .collect()
}

fn mount(container: &Container, routes: &mut HttpRoutes, path: &str, detached: &DetachedWork) {
    let hosts = hosts_on(container, path);

    let mut index = MountIndex::default();
    for (position, host) in hosts.iter().enumerate() {
        index.hosts.push(host.type_name);
        let declared = host.declared_routes();
        let names: Vec<String> = declared.tool_names().collect();
        tracing::info!(
            target: nest_rs_http::target::ROUTES,
            kind = MCP_LABEL,
            path,
            host = host.host(),
            tools = names.join(", ").as_str(),
            "mounted mcp host",
        );
        index.tools.read(position, names, declared.complete);
        index
            .prompts
            .read(position, declared.prompt_names(), declared.complete);
    }

    // A contested or unbacked identity already failed the boot in `check_identity`.
    let identity = Arc::new(resolve_identity(container, path, &hosts).unwrap_or_default());
    let window = container
        .get::<nest_rs_http::HttpConfig>()
        .unwrap_or_default()
        .shutdown_timeout;
    container
        .get::<crate::McpConfig>()
        .unwrap_or_default()
        .warn_when_operations_outlast_the_window(window, path);
    let index = Arc::new(index);
    let mount = McpMount::from_container(container)
        .stopped_with(detached.clone())
        .declaring(index.clone());
    let shared_path: Arc<str> = Arc::from(path);
    let container = container.clone();
    routes.route_any(
        path,
        endpoint(mount, move || {
            CompositeHandler::build(
                &container,
                shared_path.clone(),
                &hosts,
                index.clone(),
                identity.clone(),
            )
        }),
    );
}

/// What the hosts on one path declare, read once at mount: rmcp's handlers
/// rebuild a whole router on every lookup.
#[derive(Debug, Default)]
pub(crate) struct MountIndex {
    /// The tools, by name.
    pub(crate) tools: RouteIndex,
    /// The prompts, by name.
    pub(crate) prompts: RouteIndex,
    /// Each host's type path, by its position.
    pub(crate) hosts: Vec<&'static str>,
}

impl MountIndex {
    /// The type path of the host declaring the tool or the prompt `name`, as
    /// `kind` says which.
    pub(crate) fn host(&self, kind: McpOperationKind, name: &str) -> Option<&'static str> {
        let routes = match kind {
            McpOperationKind::Tool => &self.tools,
            McpOperationKind::Prompt => &self.prompts,
        };
        routes
            .owner(name)
            .and_then(|position| self.hosts.get(position).copied())
    }
}

/// The names one kind of route — a tool, a prompt — the hosts on one path
/// declare.
#[derive(Debug, Default)]
pub(crate) struct RouteIndex {
    /// Name → position in the path's host list.
    owners: HashMap<String, usize>,
    /// The positions of the hosts whose routes no router says, which only
    /// their own handler can name.
    unread: Vec<usize>,
}

impl RouteIndex {
    /// Record the host at `position` declaring `names`, every one it serves
    /// when `complete`; a host neither declares nor says is unread.
    pub(crate) fn read(
        &mut self,
        position: usize,
        names: impl IntoIterator<Item = String>,
        complete: bool,
    ) {
        let mut any = false;
        for name in names {
            any = true;
            // A duplicate already failed the boot.
            self.owners.entry(name).or_insert(position);
        }
        if !any && !complete {
            self.unread.push(position);
        }
    }

    /// The position of the host declaring `name`.
    pub(crate) fn owner(&self, name: &str) -> Option<usize> {
        self.owners.get(name).copied()
    }

    /// The positions of the hosts the index could not read.
    pub(crate) fn unread(&self) -> &[usize] {
        &self.unread
    }

    /// Whether a host on the path declares `name`; `None` when only a host
    /// the index could not read can say.
    pub(crate) fn declares(&self, name: &str) -> Option<bool> {
        if self.owners.contains_key(name) {
            Some(true)
        } else if self.unread.is_empty() {
            Some(false)
        } else {
            None
        }
    }
}

/// Fail boot when two hosts on one path serve the same tool name. A host whose
/// router the boot cannot read is probed with `get_tool` for every candidate.
fn check_duplicate_tools(path: &str, hosts: &[ResolvedHost]) -> Result<(), String> {
    let candidates: BTreeSet<&String> = hosts.iter().flat_map(|host| host.tools.iter()).collect();

    let mut owners: BTreeMap<&String, Vec<&'static str>> = BTreeMap::new();
    for name in candidates {
        for host in hosts {
            let owns = if host.reads_tools() {
                host.tools.contains(name)
            } else {
                host.instance.get_tool(name).is_some()
            };
            if owns {
                owners.entry(name).or_default().push(host.name);
            }
        }
    }
    refuse_clashes(path, "tool", owners)
}

/// Fail boot when two hosts on one path declare the same prompt name: the
/// index would route it to the first, the second's unreachable.
fn check_duplicate_prompts(path: &str, hosts: &[ResolvedHost]) -> Result<(), String> {
    let mut owners: BTreeMap<&String, Vec<&'static str>> = BTreeMap::new();
    for host in hosts {
        for name in &host.prompts {
            owners.entry(name).or_default().push(host.name);
        }
    }
    refuse_clashes(path, "prompt", owners)
}

/// The boot failure naming every `kind` name more than one host claims.
fn refuse_clashes(
    path: &str,
    kind: &str,
    owners: BTreeMap<&String, Vec<&'static str>>,
) -> Result<(), String> {
    let clashes: Vec<String> = owners
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(name, owners)| format!("{name} ({})", owners.join(" and ")))
        .collect();

    if clashes.is_empty() {
        return Ok(());
    }
    Err(format!(
        "duplicate MCP {kind} name on {path:?}: {} — a {kind} is addressed by bare \
         name within an endpoint, so rename one of them or give each host its \
         own path",
        clashes.join(", "),
    ))
}

/// Report a host that shares a path but declares no tool name the boot check can
/// see (a router under another name): a clash between two such hosts is
/// invisible to [`check_duplicate_tools`].
fn warn_undeclared_tools(path: &str, hosts: &[ResolvedHost]) {
    let opaque: Vec<&'static str> = hosts
        .iter()
        .filter(|host| !host.reads_tools())
        .map(|host| host.name)
        .collect();
    if opaque.is_empty() {
        return;
    }
    tracing::warn!(
        target: crate::TARGET,
        path,
        hosts = opaque.join(", ").as_str(),
        reason = "tools_not_statically_declared",
        hint = "name the router `tool_router` so the duplicate-tool boot check can see its names",
        "mcp hosts on a shared path declare no tool names the boot check can read",
    );
}

/// Settle what the endpoint at `path` calls itself: two hosts declaring it fail
/// the boot; nobody naming it is a `warn`, compared against rmcp's own
/// `ServerConfig::new` default rather than a literal.
fn check_identity(container: &Container, path: &str, hosts: &[ResolvedHost]) -> Result<(), String> {
    let metas: Vec<Arc<McpHostMeta>> = hosts.iter().map(|host| host.meta.clone()).collect();
    let identity = resolve_identity(container, path, &metas)?;
    if identity.implementation().is_some() {
        return Ok(());
    }
    let Some(first) = hosts.first() else {
        return Ok(());
    };

    let sdk_default = ServerConfig::new(ServerCapabilities::default()).server_info;
    let reported = first.instance.get_info().server_info;
    let names = hosts
        .iter()
        .map(|host| host.name)
        .collect::<Vec<_>>()
        .join(", ");
    let hint = "name the app once: McpModule::for_root(McpOptions { \
                server: Some(McpIdentity::new(name, env!(\"CARGO_PKG_VERSION\"))), \
                ..Default::default() })";

    if reported == sdk_default {
        tracing::warn!(
            target: crate::TARGET,
            path,
            hosts = names.as_str(),
            reports_as = reported.name.as_str(),
            reason = "endpoint_identity_is_the_sdk_default",
            hint,
            "an MCP endpoint introduces itself with the SDK's own name and version — neither its hosts nor the app named it",
        );
        return Ok(());
    }

    if hosts.len() > 1 {
        tracing::warn!(
            target: crate::TARGET,
            path,
            hosts = names.as_str(),
            reports_as = reported.name.as_str(),
            reason = "endpoint_identity_undeclared",
            hint,
            "several MCP hosts share an endpoint whose identity nobody declared — it reports the first host's",
        );
    }
    Ok(())
}

/// Fail boot when the app named a server no `#[mcp]` host serves.
pub(crate) fn server_reaches_a_host(container: &Container) -> anyhow::Result<()> {
    if !Discovery::new(container).meta::<McpHostMeta>().is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "an MCP server identity is declared but no #[mcp] host serves anything, so the \
         declaration reaches nothing: import the module owning your tool host, or drop \
         `server` from McpOptions"
    )
}

/// Fail boot when hosts sharing a path support no protocol version in common,
/// and warn when they merely disagree.
fn check_protocol_versions(path: &str, hosts: &[ResolvedHost]) -> Result<(), String> {
    let declared: Vec<(&'static str, Cow<'static, [ProtocolVersion]>)> = hosts
        .iter()
        .map(|host| (host.name, host.instance.supported_protocol_versions()))
        .collect();

    let lists: Vec<Cow<'static, [ProtocolVersion]>> =
        declared.iter().map(|(_, list)| list.clone()).collect();
    if lists.windows(2).all(|pair| pair[0] == pair[1]) {
        return Ok(());
    }

    let names = || {
        declared
            .iter()
            .map(|(host, _)| *host)
            .collect::<Vec<_>>()
            .join(", ")
    };

    if common_protocol_versions(&lists).is_empty() {
        return Err(format!(
            "MCP hosts on {path:?} support no protocol version in common: {} — \
             an endpoint negotiates one version for every host that shares it",
            names(),
        ));
    }

    tracing::warn!(
        target: crate::TARGET,
        path,
        hosts = names().as_str(),
        reason = "protocol_version_disagreement",
        "mcp hosts on one path declare different protocol versions — the endpoint advertises their intersection",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_is_addressed_where_register_host_mounts_it() {
        for (declared, mounted) in [
            ("", DEFAULT_PATH),
            ("/mcp/", "/mcp"),
            ("/mcp/markers", "/mcp/markers"),
            ("/", "/"),
        ] {
            assert_eq!(mount_address(declared), mounted, "declared {declared:?}");
            let defaulted = if declared.is_empty() {
                DEFAULT_PATH
            } else {
                declared
            };
            assert_eq!(
                normalize_mount_path(defaulted),
                mounted,
                "declared {declared:?}"
            );
        }
    }
}

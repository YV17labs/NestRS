//! The per-path host registry: [`register_host`] records an [`McpHostMeta`] per
//! `#[mcp]` host and, for the **first** host on a path, attaches the one
//! [`HttpEndpointMeta`] that mounts them all, merged into a [`CompositeHandler`].
//!
//! Metadata, not `inventory`: it is attached from `Discoverable::register`, so a
//! host whose module the app does not import contributes nothing. The guard, the
//! tool context, [`McpConfig`](crate::McpConfig) and the session store resolve
//! once per path ([`McpMount::from_container`]).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use nest_rs_core::{Container, ContainerBuilder, Discovery};
use nest_rs_http::{DetachedWork, HttpBootCheck, HttpEndpointMeta, normalize_mount_path};
use poem::Route;
use rmcp::model::{ProtocolVersion, ServerCapabilities, ServerConfig, Tool};

use crate::composite::{CompositeHandler, common_protocol_versions};
use crate::endpoint::{McpMount, endpoint};
use crate::host::McpHost;
use crate::identity::{McpIdentity, ResolvedIdentity};

/// The `HttpEndpointMeta` label every MCP mount carries.
pub(crate) const MCP_LABEL: &str = "mcp";

/// Where a bare `#[mcp]` mounts; a host that wants its own endpoint writes the
/// whole path instead.
pub const DEFAULT_PATH: &str = "/mcp";

/// Build one host instance from the live container, per MCP session.
type BuildHost = fn(&Container) -> Arc<dyn McpHost>;

/// The tools a host declares *statically* — its `#[tool_router]`, evaluated
/// without an instance, for boot-time duplicate detection.
type StaticTools = fn() -> Vec<Tool>;

/// One `#[mcp]` host's contribution to the endpoint at its path.
pub struct McpHostMeta {
    path: String,
    host: &'static str,
    identity: McpIdentity,
    build: BuildHost,
    tools: StaticTools,
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
        (self.tools)()
    }

    pub(crate) fn build(&self, container: &Container) -> Arc<dyn McpHost> {
        (self.build)(container)
    }
}

/// Empty fallback so `#[mcp]` can ask **any** host for its declared tools: an
/// inherent `tool_router()` wins over this trait's.
///
/// **Internal ABI** — named by the `#[mcp]` expansion, lockstep with this
/// crate; never implemented by hand.
#[doc(hidden)]
pub trait DefaultToolRouter: Sized + Send + Sync + 'static {
    /// No router of its own.
    fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        rmcp::handler::server::router::tool::ToolRouter::default()
    }
}

impl<T: Send + Sync + 'static> DefaultToolRouter for T {}

/// Empty fallback so the struct-level `#[mcp]` can ask **any** host which layers
/// its operations declared; the `#[tools]` impl's inherent function wins. One
/// tuple, so keys and names stay index-aligned.
///
/// **Internal ABI** — named by the `#[mcp]` expansion, lockstep with this
/// crate; never implemented by hand.
#[doc(hidden)]
pub trait DefaultOperationLayers: Sized + 'static {
    /// No decorated operations, so no per-operation layers.
    fn __nestrs_mcp_operation_layers() -> (Vec<std::any::TypeId>, Vec<&'static str>) {
        (Vec::new(), Vec::new())
    }
}

impl<T: 'static> DefaultOperationLayers for T {}

/// Record `P` as a host of the MCP endpoint at `path`, and — for the first host
/// to claim that path — attach the endpoint itself plus its boot check.
///
/// **Internal ABI** — called by the `#[mcp]` expansion, lockstep with this
/// crate.
#[doc(hidden)]
pub fn register_host<P: 'static>(
    builder: ContainerBuilder,
    path: &'static str,
    host: &'static str,
    identity: McpIdentity,
    build: BuildHost,
    tools: StaticTools,
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

    let builder = builder.attach_meta::<P, McpHostMeta>(McpHostMeta {
        path: path.clone(),
        host,
        identity,
        build,
        tools,
    });

    if claimed {
        return builder;
    }

    let mount_path = path.clone();
    let check_path = path.clone();
    let detached = DetachedWork::new();
    let mount_work = detached.clone();
    builder
        .attach_meta::<P, HttpEndpointMeta>(
            HttpEndpointMeta::new(path, MCP_LABEL, move |container, route: Route| {
                mount(container, route, &mount_path, &mount_work)
            })
            .runs_detached(detached)
            .owned_by(host)
            .exempt(),
        )
        .attach_meta::<P, HttpBootCheck>(HttpBootCheck::new(move |container| {
            let path = check_path.as_str();
            let hosts = resolve(container, path);
            check_identity(container, path, &hosts)?;
            if hosts.len() < 2 {
                return Ok(());
            }
            check_duplicate_tools(path, &hosts)?;
            warn_undeclared_tools(path, &hosts);
            check_protocol_versions(path, &hosts)
        }))
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
        declared_server(container).as_deref(),
    )
    .map_err(|err| err.report(path, host.map_or("<none>", |meta| meta.host())))
}

/// The app's own [`McpIdentity`], if it declared one through
/// [`McpOptions::server`](crate::McpOptions::server); two that disagree fail the boot.
fn declared_server(container: &Container) -> Option<Arc<McpIdentity>> {
    Discovery::new(container)
        .meta::<McpIdentity>()
        .into_iter()
        .map(|discovered| discovered.meta)
        .next()
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
/// check: `build` runs the developer's constructor and `declared_tools` builds a
/// whole `ToolRouter`.
struct ResolvedHost {
    meta: Arc<McpHostMeta>,
    name: &'static str,
    declared: BTreeSet<String>,
    instance: Arc<dyn McpHost>,
}

fn resolve(container: &Container, path: &str) -> Vec<ResolvedHost> {
    hosts_on(container, path)
        .into_iter()
        .map(|meta| ResolvedHost {
            name: meta.host(),
            declared: declared_names(&meta).map(Cow::into_owned).collect(),
            instance: meta.build(container),
            meta,
        })
        .collect()
}

fn declared_names(meta: &McpHostMeta) -> impl Iterator<Item = Cow<'static, str>> + use<> {
    meta.declared_tools().into_iter().map(|tool| tool.name)
}

fn mount(container: &Container, route: Route, path: &str, detached: &DetachedWork) -> Route {
    let hosts = hosts_on(container, path);

    // Built once: rmcp's `#[tool_handler]` rebuilds the whole `ToolRouter` on every `get_tool`.
    let mut tools = ToolIndex::new();
    for (position, host) in hosts.iter().enumerate() {
        let names: Vec<Cow<'static, str>> = declared_names(host).collect();
        tracing::info!(
            target: nest_rs_http::target::ROUTES,
            kind = MCP_LABEL,
            path,
            host = host.host(),
            tools = names.join(", ").as_str(),
            "mounted mcp host",
        );
        for name in names {
            // A duplicate already failed the boot.
            tools.entry(name.into_owned()).or_insert(position);
        }
    }

    // A contested or unbacked identity already failed the boot in `check_identity`.
    let identity = Arc::new(resolve_identity(container, path, &hosts).unwrap_or_default());
    let mount = McpMount::from_container(container).stopped_with(detached.clone());
    let tools = Arc::new(tools);
    let shared_path: Arc<str> = Arc::from(path);
    let container = container.clone();
    route.nest(
        path,
        endpoint(mount, move || {
            CompositeHandler::build(
                &container,
                shared_path.clone(),
                &hosts,
                tools.clone(),
                identity.clone(),
            )
        }),
    )
}

/// Tool name → position in the path's host list.
pub(crate) type ToolIndex = std::collections::HashMap<String, usize>;

/// Fail boot when two hosts on one path serve the same tool name. A host that
/// declares no `#[tool_router]` is probed with `get_tool` for every candidate.
fn check_duplicate_tools(path: &str, hosts: &[ResolvedHost]) -> Result<(), String> {
    let candidates: BTreeSet<&String> =
        hosts.iter().flat_map(|host| host.declared.iter()).collect();

    let mut owners: BTreeMap<&String, Vec<&'static str>> = BTreeMap::new();
    for name in candidates {
        for host in hosts {
            let owns = if host.declared.is_empty() {
                host.instance.get_tool(name).is_some()
            } else {
                host.declared.contains(name)
            };
            if owns {
                owners.entry(name).or_default().push(host.name);
            }
        }
    }

    let clashes: Vec<String> = owners
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(name, owners)| format!("{name} ({})", owners.join(" and ")))
        .collect();

    if clashes.is_empty() {
        return Ok(());
    }
    Err(format!(
        "duplicate MCP tool name on {path:?}: {} — a tool is addressed by bare \
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
        .filter(|host| host.declared.is_empty())
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
pub(crate) fn server_reaches_a_host() -> HttpBootCheck {
    HttpBootCheck::new(|container| {
        if !Discovery::new(container).meta::<McpHostMeta>().is_empty() {
            return Ok(());
        }
        Err(
            "an MCP server identity is declared but no #[mcp] host serves anything, so the \
             declaration reaches nothing: import the module owning your tool host, or drop \
             `server` from McpOptions"
                .to_owned(),
        )
    })
}

/// Fail boot when two imports declare a different server for one app.
pub(crate) fn server_is_declared_once() -> HttpBootCheck {
    HttpBootCheck::new(|container| {
        let declared = Discovery::new(container).meta::<McpIdentity>();
        let mut seen: Option<&Arc<McpIdentity>> = None;
        for identity in declared.iter().map(|discovered| &discovered.meta) {
            match seen {
                // Dynamic module registration is not deduplicated: one `for_root` twice is fine.
                Some(first) if first.as_ref() == identity.as_ref() => {}
                Some(first) => {
                    return Err(format!(
                        "two different MCP server identities are declared: {first:?} and \
                         {identity:?} — an app reports one serverInfo, so declare it once \
                         through a single McpModule::for_root",
                    ));
                }
                None => seen = Some(identity),
            }
        }
        Ok(())
    })
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

//! The per-path host registry — what a `#[mcp]` provider contributes, and the
//! single mount it contributes *to*. The contribution itself is an
//! [`McpHost`](crate::McpHost); this module is where several of them become one
//! endpoint.
//!
//! # One mount, several providers
//!
//! Every other transport in the framework aggregates: controllers mount routes
//! flat into one `Route`, resolvers merge into one schema, `#[process]` methods
//! collect per queue name. MCP used to be the exception — one `#[mcp(path)]`
//! owned the whole `ServerHandler` — and that exception was load-bearing in the
//! wrong direction: the MCP spec namespaces tools **per endpoint** and every
//! shipped client config points at a single URL, so a product exposing several
//! domains over MCP had to fold them into one god-host, inverting the
//! one-adapter-per-feature layout the rules mandate.
//!
//! Now a `#[mcp]` host is a *contribution*: [`register_host`] records an
//! [`McpHostMeta`] for it and, for the **first** host on a given path, attaches
//! the one [`HttpEndpointMeta`] that mounts them all. At mount time the
//! contributions for that path are merged into a
//! [`CompositeHandler`](crate::CompositeHandler).
//!
//! # Why metadata rather than `inventory`
//!
//! GraphQL merges a link-time `inventory` registry and has to filter it against
//! `ReachableProviders`, because linking is not importing. Metadata is attached
//! from `Discoverable::register`, which only ever runs for a provider a module
//! in the running app's import graph actually registers — so module-gating here
//! is **structural**: a `#[mcp]` host whose module the app does not import
//! contributes nothing, with nothing to filter and nothing to warn about.
//!
//! # What resolves once per path
//!
//! The operation guard, the `dyn McpToolContext`, [`McpConfig`](crate::McpConfig)
//! and any session store are container bindings, so [`McpMount::from_container`]
//! resolves them **once for the path** rather than once per host — two modules
//! on one path cannot disagree about the posture of the endpoint they share.
//! Two modules binding *different* `dyn McpToolContext` implementations is a
//! container-level override (`provide_dyn`, last binding wins), which is the
//! same answer the framework gives everywhere else and not a per-path decision.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use nest_rs_core::{Container, ContainerBuilder, DiscoveryService};
use nest_rs_http::{HttpBootCheck, HttpEndpointMeta};
use poem::Route;
use rmcp::model::{ProtocolVersion, ServerCapabilities, ServerInfo, Tool};

use crate::composite::{CompositeHandler, common_protocol_versions};
use crate::endpoint::{McpMount, endpoint};
use crate::host::McpHost;
use crate::identity::McpEndpoint;

/// The `HttpEndpointMeta` label every MCP mount carries — what the transport's
/// boot log prints and what [`register_host`] matches on to find a path already
/// claimed by a peer host.
pub(crate) const MCP_LABEL: &str = "mcp";

/// Build one host instance from the live container. Runs per MCP session, the
/// same lifetime the single-host factory had.
type BuildHost = fn(&Container) -> Arc<dyn McpHost>;

/// The tools a host declares *statically* — its `#[tool_router]`, evaluated
/// without an instance. Boot-time duplicate detection needs candidate names,
/// and `ServerHandler::get_tool` can only answer about a name you already have.
type StaticTools = fn() -> Vec<Tool>;

/// One `#[mcp]` host's contribution to the endpoint at its path.
///
/// Attached to the host provider by [`register_host`]; read back at mount and
/// at boot through [`DiscoveryService::meta`].
pub struct McpHostMeta {
    path: Cow<'static, str>,
    host: &'static str,
    build: BuildHost,
    tools: StaticTools,
}

impl McpHostMeta {
    /// The endpoint path this host contributes to.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The host struct's name (`PostsTool`) — what a boot log and a duplicate
    /// report name.
    pub fn host(&self) -> &'static str {
        self.host
    }

    /// The tools this host declares through rmcp's `#[tool_router]`. Empty for
    /// a host that hand-writes `list_tools`/`call_tool` or keeps its router
    /// under another name: those still serve, they just contribute no
    /// *statically* known names — see [`warn_undeclared_tools`].
    pub fn declared_tools(&self) -> Vec<Tool> {
        (self.tools)()
    }

    /// Build one live instance of this host — per MCP session at the mount,
    /// once per boot pass in the checks.
    pub(crate) fn build(&self, container: &Container) -> Arc<dyn McpHost> {
        (self.build)(container)
    }
}

/// Empty-router fallback so `#[mcp]` can ask **any** host for its declared
/// tools.
///
/// rmcp's `#[tool_router]` emits an *inherent* `fn tool_router() -> ToolRouter<Self>`,
/// and an inherent associated function wins over a trait one, so
/// `<Host>::tool_router()` in the `#[mcp]` expansion resolves to the real
/// router when there is one and to this empty stand-in when there is not. That
/// is what lets the decorator sit on a hand-written `ServerHandler` without
/// requiring the developer to declare anything.
///
/// **Internal ABI** — named by the `#[mcp]` expansion, lockstep with this
/// crate; never implemented by hand.
#[doc(hidden)]
pub trait DefaultToolRouter: Sized + Send + Sync + 'static {
    /// No statically-known tools.
    fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        rmcp::handler::server::router::tool::ToolRouter::default()
    }
}

impl<T: Send + Sync + 'static> DefaultToolRouter for T {}

/// Record `P` as a host of the MCP endpoint at `path`, and — for the first host
/// to claim that path — attach the endpoint itself plus its boot check.
///
/// This is what `#[mcp]`'s `Discoverable::register` calls. The resolution order
/// and the merge policy live here, in the crate, rather than inside a macro
/// expansion, so they are readable and testable.
///
/// **Internal ABI** — called by the `#[mcp]` expansion, lockstep with this
/// crate.
#[doc(hidden)]
pub fn register_host<P: 'static>(
    builder: ContainerBuilder,
    path: &'static str,
    host: &'static str,
    build: BuildHost,
    tools: StaticTools,
) -> ContainerBuilder {
    // A peer host already mounted this path: this one only adds its
    // contribution. Exactly one `HttpEndpointMeta` per path is what keeps the
    // transport's "a mount path is its owner's exclusive namespace" rule intact
    // — a genuine collision (an `#[mcp]` and a `#[gateway]` on one path) still
    // fails boot naming both.
    //
    // Which host claims the path is *not* a decision: the mount closure reads
    // every contributor back from the container, so all of them are equivalent.
    // The deeper form — `HttpEndpointMeta` gaining an "aggregated mount" the
    // transport merges itself, so every host attaches unconditionally — is
    // deliberately not built for one caller: `CLAUDE.md` says extract after a
    // pattern appears twice, and MCP is the only aggregating self-mount today.
    // A second one (the WS "route by event name" change `framework.md`
    // anticipates) is what should generalize this.
    let claimed = builder
        .attached_meta::<HttpEndpointMeta>()
        .any(|meta| meta.label() == MCP_LABEL && meta.path() == path);

    let builder = builder.attach_meta::<P, McpHostMeta>(McpHostMeta {
        path: Cow::Borrowed(path),
        host,
        build,
        tools,
    });

    if claimed {
        return builder;
    }

    builder
        .attach_meta::<P, HttpEndpointMeta>(
            HttpEndpointMeta::new(path, MCP_LABEL, move |container, route: Route| {
                mount(container, route, path)
            })
            // A cross-family collision reports by owner, and "mcp endpoint mcp"
            // is the degenerate message `owned_by` exists to prevent. The
            // claiming host is the honest name: it is the one whose module
            // brought the endpoint into the app.
            .owned_by(host)
            .exempt(),
        )
        .attach_meta::<P, HttpBootCheck>(HttpBootCheck::new(move |container| {
            // One resolution pass for every check: each host is constructed
            // once and asked for its router once, however many questions the
            // boot has about the path.
            let hosts = resolve(container, path);
            // Identity is a question every endpoint answers, merged or not.
            warn_undeclared_identity(container, path, &hosts);
            if hosts.len() < 2 {
                return Ok(());
            }
            check_duplicate_tools(path, &hosts)?;
            warn_undeclared_tools(path, &hosts);
            check_protocol_versions(path, &hosts)
        }))
}

/// The identity the app declared for `path`, if it declared one.
///
/// Provider-less metadata from [`McpModule::endpoint`](crate::McpModule::endpoint)
/// — the app's statement about the endpoint, read here and by the mount.
pub fn declared_endpoint(container: &Container, path: &str) -> Option<Arc<McpEndpoint>> {
    DiscoveryService::new(container)
        .meta::<McpEndpoint>()
        .into_iter()
        .map(|discovered| discovered.meta)
        .find(|endpoint| endpoint.path() == path)
}

/// Every host contributing to `path`, in registration order — what the mount
/// merges, and what a test or a diagnostic asks to see the composition of an
/// endpoint.
pub fn hosts_on(container: &Container, path: &str) -> Vec<Arc<McpHostMeta>> {
    DiscoveryService::new(container)
        .meta::<McpHostMeta>()
        .into_iter()
        .map(|discovered| discovered.meta)
        .filter(|meta| meta.path() == path)
        .collect()
}

/// One host on a path, resolved as far as the boot checks need it.
///
/// Resolved **once** per boot pass and shared by every check. Neither half is
/// free: `build` runs the developer's own constructor, and `declared_tools`
/// makes rmcp assemble a whole `ToolRouter` (every tool's schema included), so
/// asking twice is work no check earns.
struct ResolvedHost {
    name: &'static str,
    /// Names from the host's `#[tool_router]`. Empty when it has none under
    /// that name — see [`warn_undeclared_tools`].
    declared: BTreeSet<String>,
    instance: Arc<dyn McpHost>,
}

fn resolve(container: &Container, path: &str) -> Vec<ResolvedHost> {
    hosts_on(container, path)
        .iter()
        .map(|meta| ResolvedHost {
            name: meta.host(),
            declared: declared_names(meta).map(Cow::into_owned).collect(),
            instance: meta.build(container),
        })
        .collect()
}

/// A host's statically declared tool names, in rmcp's own (sorted) order.
fn declared_names(meta: &McpHostMeta) -> impl Iterator<Item = Cow<'static, str>> + use<> {
    meta.declared_tools().into_iter().map(|tool| tool.name)
}

/// Mount the endpoint for `path`: merge every host that contributes to it into
/// one handler, behind the guard/context/config resolved once for the path.
fn mount(container: &Container, route: Route, path: &'static str) -> Route {
    let hosts = hosts_on(container, path);

    // Tool name → the position of the host that declares it. Built here, once,
    // because rmcp's `#[tool_handler]` rebuilds the whole `ToolRouter` on every
    // `get_tool` call: routing `tools/call` by asking each host in turn would
    // re-assemble every router, with every tool's schema, on every invocation.
    let mut tools = ToolIndex::new();
    for (position, host) in hosts.iter().enumerate() {
        let names: Vec<Cow<'static, str>> = declared_names(host).collect();
        tracing::info!(
            target: "nest_rs::routes",
            kind = MCP_LABEL,
            path,
            host = host.host(),
            tools = names.join(", ").as_str(),
            "mounted mcp host",
        );
        for name in names {
            // A duplicate would have failed boot already, so first-wins here is
            // recording an order that cannot exist rather than resolving one.
            tools.entry(name.into_owned()).or_insert(position);
        }
    }

    let identity = declared_endpoint(container, path);
    let mount = McpMount::from_container(container);
    let tools = Arc::new(tools);
    let container = container.clone();
    route.nest(
        path,
        endpoint(mount, move || {
            CompositeHandler::build(&container, path, &hosts, tools.clone(), identity.clone())
        }),
    )
}

/// Tool name → position in the path's host list. See [`mount`].
pub(crate) type ToolIndex = std::collections::HashMap<String, usize>;

/// Fail boot when two hosts on one path serve the same tool name.
///
/// The merge is what makes this expressible at all: one host per path could not
/// collide with anyone. A collision is not a style nit — MCP addresses a tool by
/// bare name within an endpoint, so the loser is simply unreachable, and which
/// one loses depends on registration order.
///
/// Candidate names come from the hosts that declare a `#[tool_router]`. A host
/// that declares none is then *probed* with `get_tool` for every candidate,
/// which is what catches the one shape static names miss — a host whose router
/// lives behind another name. A host that already declared its names answers
/// exactly that set, so probing it would only rebuild its router for nothing.
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
/// see.
///
/// `#[mcp]` reads a host's tools through rmcp's default `tool_router()`. A host
/// that keeps its router behind another name — `#[tool_router(router = …)]`
/// plus a field, which rmcp itself documents for a host with many tools — still
/// *serves* every tool, but contributes no candidate names, so a clash between
/// two such hosts is invisible to [`check_duplicate_tools`]. That is a real gap
/// in the one invariant the merge introduces, and it is said out loud rather
/// than left to be discovered on the wire. Alone on a path there is nothing to
/// clash with, so this is silent for the ordinary single-host mount.
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
        target: "nest_rs::mcp",
        path,
        hosts = opaque.join(", ").as_str(),
        reason = "tools_not_statically_declared",
        hint = "name the router `tool_router` so the duplicate-tool boot check can see its names",
        "mcp hosts on a shared path declare no tool names the boot check can read",
    );
}

/// Report an endpoint that introduces itself as something other than the app.
///
/// One endpoint reports one `serverInfo` and one `instructions` — the
/// protocol's shape, and every SDK's. Undeclared, two things can be wrong with
/// what a client reads, and they are different facts:
///
/// * **Several hosts, no declaration.** The endpoint borrows the first host's
///   identity, so its name is a function of `imports = [..]` order and the model
///   reads one feature's instructions for the whole surface.
/// * **Nobody named it at all.** A host that does not override `get_info` gets
///   rmcp's `ServerInfo::new`, whose `server_info` is the **SDK's own**
///   build identity — so the endpoint tells every client it is `rmcp`, at rmcp's
///   version. Compared against that same constructor rather than a literal, so
///   the check cannot drift from the SDK it describes.
///
/// A lone host that named itself is a complete server and says nothing here:
/// that is the shape every MCP SDK builds.
fn warn_undeclared_identity(container: &Container, path: &str, hosts: &[ResolvedHost]) {
    if declared_endpoint(container, path).is_some() {
        return;
    }
    let Some(first) = hosts.first() else {
        return;
    };
    let names = || {
        hosts
            .iter()
            .map(|host| host.name)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let hint = "declare it: McpModule::endpoint(McpEndpoint::new(path, name, version))";

    if hosts.len() > 1 {
        tracing::warn!(
            target: "nest_rs::mcp",
            path,
            hosts = names().as_str(),
            reports_as = first.name,
            reason = "endpoint_identity_undeclared",
            hint,
            "several MCP hosts share an endpoint whose identity nobody declared — it reports the first host's",
        );
        return;
    }

    let sdk_default = ServerInfo::new(ServerCapabilities::default()).server_info;
    let reported = first.instance.get_info().server_info;
    if reported != sdk_default {
        return;
    }
    tracing::warn!(
        target: "nest_rs::mcp",
        path,
        host = first.name,
        reports_as = reported.name.as_str(),
        reason = "endpoint_identity_is_the_sdk_default",
        hint,
        "an MCP endpoint introduces itself with the SDK's own name and version — neither the host nor the app named it",
    );
}

/// Fail boot on a declaration that reaches nothing, or two that disagree.
///
/// Attached once by [`McpModule`](crate::McpModule) rather than per path,
/// because the failures it catches are about paths that have **no** host — a
/// typo in a declared path is otherwise a silent no-op, and silence is the one
/// answer a declaration must never get.
///
/// A dynamic module can be imported twice (`DynamicModule` registration is not
/// deduplicated), so two *equal* declarations for one path are that, not a
/// conflict; two that disagree are a real one.
pub(crate) fn declaration_check() -> HttpBootCheck {
    HttpBootCheck::new(|container| {
        let declared = DiscoveryService::new(container).meta::<McpEndpoint>();

        let mut by_path: BTreeMap<&str, &Arc<McpEndpoint>> = BTreeMap::new();
        for endpoint in declared.iter().map(|discovered| &discovered.meta) {
            match by_path.insert(endpoint.path(), endpoint) {
                Some(previous) if previous.as_ref() != endpoint.as_ref() => {
                    return Err(format!(
                        "two different MCP endpoint identities declared for {:?}: {:?} and {:?} \
                         — an endpoint reports one serverInfo, so declare it once",
                        endpoint.path(),
                        previous.implementation().name,
                        endpoint.implementation().name,
                    ));
                }
                _ => {}
            }
        }

        let orphans: Vec<&str> = by_path
            .keys()
            .copied()
            .filter(|path| hosts_on(container, path).is_empty())
            .collect();
        if orphans.is_empty() {
            return Ok(());
        }
        Err(format!(
            "MCP endpoint identity declared for {} — no #[mcp] host contributes to that path, so \
             the declaration reaches nothing: fix the path or import the module owning the host",
            orphans
                .iter()
                .map(|path| format!("{path:?}"))
                .collect::<Vec<_>>()
                .join(", "),
        ))
    })
}

/// Fail boot when hosts sharing a path support no protocol version in common,
/// and warn when they merely disagree.
///
/// One endpoint negotiates one version, so the merged handler advertises the
/// intersection — computed by the same [`common_protocol_versions`] the handler
/// uses, so the boot verdict and the runtime answer cannot drift. An empty
/// intersection is an endpoint that can complete no handshake at all: a boot
/// error, not a runtime mystery.
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
        target: "nest_rs::mcp",
        path,
        hosts = names().as_str(),
        reason = "protocol_version_disagreement",
        "mcp hosts on one path declare different protocol versions — the endpoint advertises their intersection",
    );
    Ok(())
}

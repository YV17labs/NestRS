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
//! # Where a host lands
//!
//! A `#[mcp]` path is the **whole URL path**, leading slash and all, exactly
//! like a `#[controller]`'s — and unlike one, nothing hangs off it. A
//! controller's path is a namespace it owns and its routes nest under; an MCP
//! host's path names *one endpoint it joins*, which is why several hosts share
//! it and why there is no prefix for it to be relative to. It should read like
//! the URL a client is configured with, because that is the only thing anyone
//! reasons about.
//!
//! [`DEFAULT_PATH`] is what a bare `#[mcp]` takes — the convention every MCP
//! client config uses, and the spelling that keeps the common case ("my feature
//! contributes tools to this app's server") free of a repeated literal. It is a
//! constant rather than a `#[config]` field: a path a *decorator* declares is
//! code everywhere in this framework (`#[controller]`, `#[gateway]`), and
//! `HttpConfig.global_prefix` already moves the whole surface when a deployment
//! needs it to.
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

use nest_rs_core::{Container, ContainerBuilder, Discovery};
use nest_rs_http::{HttpBootCheck, HttpEndpointMeta, normalize_mount_path};
use poem::Route;
use rmcp::model::{ProtocolVersion, ServerCapabilities, ServerInfo, Tool};

use crate::composite::{CompositeHandler, common_protocol_versions};
use crate::endpoint::{McpMount, endpoint};
use crate::host::McpHost;
use crate::identity::{McpIdentity, ResolvedIdentity};

/// The `HttpEndpointMeta` label every MCP mount carries — what the transport's
/// boot log prints and what [`register_host`] matches on to find a path already
/// claimed by a peer host.
pub(crate) const MCP_LABEL: &str = "mcp";

/// Where a bare `#[mcp]` mounts. The convention every shipped MCP client config
/// uses; a host that wants its own endpoint writes the whole path instead.
pub const DEFAULT_PATH: &str = "/mcp";

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
/// at boot through [`Discovery::meta`].
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

    /// What this host declares about the endpoint it serves. Empty for the
    /// ordinary host that leaves identity to the app.
    pub fn identity(&self) -> &McpIdentity {
        &self.identity
    }

    /// The host struct's name (`PostsTool`) — what a boot log and a duplicate
    /// report name.
    pub fn host(&self) -> &'static str {
        self.host
    }

    /// The tools this host declares through rmcp's `#[tool_router]`. Empty for
    /// a host that hand-writes `list_tools`/`call_tool` or keeps its router
    /// under another name: those still serve, they just contribute no
    /// *statically* known names — see `warn_undeclared_tools`.
    pub fn declared_tools(&self) -> Vec<Tool> {
        (self.tools)()
    }

    /// Build one live instance of this host — per MCP session at the mount,
    /// once per boot pass in the checks.
    pub(crate) fn build(&self, container: &Container) -> Arc<dyn McpHost> {
        (self.build)(container)
    }
}

/// Empty fallback so `#[mcp]` can ask **any** host for its declared tools.
///
/// An inherent associated function wins over a trait one, so
/// `<Host>::tool_router()` in the `#[mcp]` expansion resolves to the real router
/// — emitted by `#[tools]` as `pub(crate)`, or written by hand with
/// rmcp's own `#[tool_router]` — and to this empty stand-in when the host has
/// neither. That is what lets the decorator sit on a hand-written
/// `ServerHandler` without requiring the developer to declare anything.
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
/// its operations declared.
///
/// Same mechanism as [`DefaultToolRouter`], for the same reason: `Discoverable`
/// is emitted by the *struct* half — a host serving a hand-written
/// `ServerHandler` has no decorated impl — while `#[use_guards]` beside a
/// `#[tool]` is known only to the *impl* half. The decorated impl emits an
/// inherent `__nestrs_mcp_operation_layers`, which wins; a host without one
/// lands here and contributes nothing.
///
/// The two halves travel as one tuple rather than two functions because the
/// access graph pairs `injected()[i]` with `injected_names()[i]`: index
/// alignment that cannot be broken by editing one of two call sites.
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
    identity: McpIdentity,
    build: BuildHost,
    tools: StaticTools,
) -> ContainerBuilder {
    // A bare `#[mcp]` declares no path and takes the default; anything else is
    // the whole URL path. The canonical form is the HTTP layer's to define —
    // `/mcp/` and `/mcp` name one endpoint, and every self-mount has to agree on
    // that before `claim_exclusive_path` compares them — so this only supplies
    // the default and defers.
    let path = normalize_mount_path(match path {
        "" => DEFAULT_PATH,
        declared => declared,
    });

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
    builder
        .attach_meta::<P, HttpEndpointMeta>(
            HttpEndpointMeta::new(path, MCP_LABEL, move |container, route: Route| {
                mount(container, route, &mount_path)
            })
            // A cross-family collision reports by owner, and "mcp endpoint mcp"
            // is the degenerate message `owned_by` exists to prevent. The
            // claiming host is the honest name: it is the one whose module
            // brought the endpoint into the app.
            .owned_by(host)
            .exempt(),
        )
        .attach_meta::<P, HttpBootCheck>(HttpBootCheck::new(move |container| {
            let path = check_path.as_str();
            // One resolution pass for every check: each host is constructed
            // once and asked for its router once, however many questions the
            // boot has about the path.
            let hosts = resolve(container, path);
            // Identity is a question every endpoint answers, merged or not.
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
///
/// The `Err` arm is a boot failure `check_identity` already raised, so the mount
/// and the tests read it through [`Result::unwrap_or_default`] — an endpoint
/// that never booted has no identity to report.
pub fn endpoint_identity(container: &Container, path: &str) -> ResolvedIdentity {
    resolve_identity(container, path, &hosts_on(container, path)).unwrap_or_default()
}

/// [`endpoint_identity`] with the failures spelled out, over a host list the
/// caller already holds.
///
/// The `Err` arm is a boot message only [`check_identity`] can act on — by the
/// time anything else asks, the boot either failed or the identity is sound — so
/// the public accessor above swallows it rather than making every caller defuse
/// an unreachable arm. Taking `hosts` rather than re-deriving it keeps the mount
/// and the boot check to one metadata scan each.
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
/// [`McpOptions::server`](crate::McpOptions::server).
///
/// Taking the first of several would put the answer back where declaring it was
/// meant to take it from — `imports = [..]` order — so disagreement is a boot
/// failure instead ([`server_is_declared_once`]). A `DynamicModule` may be
/// imported twice (registration is not deduplicated), so two *equal*
/// declarations are that, not a conflict.
fn declared_server(container: &Container) -> Option<Arc<McpIdentity>> {
    Discovery::new(container)
        .meta::<McpIdentity>()
        .into_iter()
        .map(|discovered| discovered.meta)
        .next()
}

/// Every host contributing to `path`, in registration order — what the mount
/// merges, and what a test or a diagnostic asks to see the composition of an
/// endpoint.
pub fn hosts_on(container: &Container, path: &str) -> Vec<Arc<McpHostMeta>> {
    Discovery::new(container)
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
    /// The contribution this was built from — carried so the identity check
    /// reads the app's and the hosts' declarations off the one scan `resolve`
    /// already paid for.
    meta: Arc<McpHostMeta>,
    name: &'static str,
    /// Names from the host's `#[tool_router]`. Empty when it has none under
    /// that name — see [`warn_undeclared_tools`].
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

/// A host's statically declared tool names, in rmcp's own (sorted) order.
fn declared_names(meta: &McpHostMeta) -> impl Iterator<Item = Cow<'static, str>> + use<> {
    meta.declared_tools().into_iter().map(|tool| tool.name)
}

/// Mount the endpoint for `path`: merge every host that contributes to it into
/// one handler, behind the guard/context/config resolved once for the path.
fn mount(container: &Container, route: Route, path: &str) -> Route {
    let hosts = hosts_on(container, path);

    // Tool name → the position of the host that declares it. Built here, once,
    // because rmcp's `#[tool_handler]` rebuilds the whole `ToolRouter` on every
    // `get_tool` call: routing `tools/call` by asking each host in turn would
    // re-assemble every router, with every tool's schema, on every invocation.
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
            // A duplicate would have failed boot already, so first-wins here is
            // recording an order that cannot exist rather than resolving one.
            tools.entry(name.into_owned()).or_insert(position);
        }
    }

    // A contested or unbacked identity failed boot in `check_identity`, so the
    // fallback here is unreachable rather than lenient: an endpoint that never
    // booted has no identity to report.
    let identity = Arc::new(resolve_identity(container, path, &hosts).unwrap_or_default());
    let mount = McpMount::from_container(container);
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
        target: crate::TARGET,
        path,
        hosts = opaque.join(", ").as_str(),
        reason = "tools_not_statically_declared",
        hint = "name the router `tool_router` so the duplicate-tool boot check can see its names",
        "mcp hosts on a shared path declare no tool names the boot check can read",
    );
}

/// Settle what the endpoint at `path` calls itself, and report the two ways it
/// can be wrong.
///
/// One endpoint reports one `serverInfo` and one `instructions` — the protocol's
/// shape, and every SDK's. So:
///
/// * **Two hosts declaring one endpoint fails boot**, naming both
///   ([`endpoint_identity`]) — otherwise the endpoint's name would be a function
///   of `imports = [..]` order, which is the accident the whole seam removes. A
///   name with no version behind it fails there too.
/// * **Nobody naming it at all is a `warn`**, because the fallback still serves:
///   a host that does not override `get_info` gets rmcp's `ServerInfo::new`,
///   whose `server_info` is the **SDK's own** build identity — so the endpoint
///   tells every client it is `rmcp`, at rmcp's version. Compared against that
///   same constructor rather than a literal, so the check cannot drift from the
///   SDK it describes.
///
/// A shared endpoint whose hosts merely *join* their instructions is not a
/// fallback and says nothing here — that is the sum of its features, which is
/// what a client should read until someone frames the whole.
fn check_identity(container: &Container, path: &str, hosts: &[ResolvedHost]) -> Result<(), String> {
    let metas: Vec<Arc<McpHostMeta>> = hosts.iter().map(|host| host.meta.clone()).collect();
    let identity = resolve_identity(container, path, &metas)?;
    if identity.implementation().is_some() {
        return Ok(());
    }
    let Some(first) = hosts.first() else {
        return Ok(());
    };

    let sdk_default = ServerInfo::new(ServerCapabilities::default()).server_info;
    let reported = first.instance.get_info().server_info;
    let names = hosts
        .iter()
        .map(|host| host.name)
        .collect::<Vec<_>>()
        .join(", ");
    let hint = "name the app once: McpModule::for_root(McpOptions { \
                server: Some(McpIdentity::new(name, env!(\"CARGO_PKG_VERSION\"))), \
                ..Default::default() })";

    // Two different facts, so two messages: each is emitted only where it is
    // literally true. One message covering both would contradict its own
    // `reports_as` field the moment a host had named itself.
    if reported == sdk_default {
        // rmcp's `ServerInfo::new` leaves the **SDK's** build identity in place,
        // so this endpoint tells every client it is `rmcp`, at rmcp's version.
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
        // Each host named itself, so the endpoint answers with whichever one
        // registered first — a function of `imports = [..]` order.
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
    // A lone host that named itself through `get_info` is a complete server —
    // the shape every MCP SDK builds, and nothing to report.
    Ok(())
}

/// Fail boot when the app named a server no `#[mcp]` host serves.
///
/// Attached by [`McpModule`](crate::McpModule) only when an identity was
/// actually declared, because the failure it catches is a declaration with
/// nothing behind it — an app that named itself and then forgot to import the
/// module owning its tools. Silence is the one answer a declaration must never
/// get, and no per-path check can see this one: there is no path.
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
///
/// The app's identity carries no path, so no per-path check can see this one:
/// without it, [`declared_server`] would take whichever `McpModule::for_root`
/// the import graph happened to reach first, and the endpoint's name would
/// silently depend on `imports = [..]` order — the accident the seam exists to
/// remove.
pub(crate) fn server_is_declared_once() -> HttpBootCheck {
    HttpBootCheck::new(|container| {
        let declared = Discovery::new(container).meta::<McpIdentity>();
        let mut seen: Option<&Arc<McpIdentity>> = None;
        for identity in declared.iter().map(|discovered| &discovered.meta) {
            match seen {
                // Importing one `for_root` twice is not a conflict: dynamic
                // module registration is deliberately not deduplicated.
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
        target: crate::TARGET,
        path,
        hosts = names().as_str(),
        reason = "protocol_version_disagreement",
        "mcp hosts on one path declare different protocol versions — the endpoint advertises their intersection",
    );
    Ok(())
}

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use nest_rs_core::{Container, Discovery, Transport};
use poem::endpoint::BoxEndpoint;
use poem::http::header::{HeaderName, HeaderValue, SERVER};
use poem::middleware::{Compression, Cors};
use poem::{EndpointExt, Response, Route};
use tokio_util::sync::CancellationToken;

use crate::boot_check::{GlobalGuardsActive, HttpBootCheck};
use crate::controller::HttpControllerMeta;
use crate::detached::DetachedWork;
use crate::drain::Drain;
use crate::endpoint::{EdgePosture, HttpEndpointMeta, SelfMountGuardWrap};
use crate::fallback::{Claims, Fallback, HttpFallbackMeta, WithFallback, literal_prefix};
use crate::interceptor::HttpEndpointWrap;
use crate::route_template::RouteTemplate;
use crate::server::Server;
use crate::tls::HttpTls;
use crate::versioning::VersionedEndpoint;

/// The `Server` value a transport sends when [`HttpConfig::server_header`](crate::HttpConfig::server_header) is on.
const SERVER_VALUE: &str = concat!("nestrs/", env!("CARGO_PKG_VERSION"));

/// Join a controller prefix with a route path the way poem's nesting does:
/// `("/health", "/live") -> "/health/live"`.
pub fn join_path(prefix: &str, rest: &str) -> String {
    let p = prefix.trim_end_matches('/');
    let r = rest.trim_start_matches('/');
    match (p.is_empty(), r.is_empty()) {
        (true, true) => "/".to_string(),
        (false, true) => p.to_string(),
        (true, false) => format!("/{r}"),
        (false, false) => format!("{p}/{r}"),
    }
}

/// Apply URI API versioning: `Some("1"), "/users"` → `"/v1/users"`.
pub fn version_path(version: Option<&str>, path: &str) -> String {
    match version {
        Some(v) => join_path(&format!("/v{v}"), path),
        None => path.to_string(),
    }
}

/// Does `declared` contain every version in `named`?
///
/// `#[routes]` asserts it in `const` per `#[version("…")]` route: the mount loops
/// over the controller's versions, so an undeclared one would compile and answer nothing.
pub const fn versions_declare(declared: &[&str], named: &[&str]) -> bool {
    let mut i = 0;
    while i < named.len() {
        let mut found = false;
        let mut j = 0;
        while j < declared.len() {
            if const_str_eq(declared[j], named[i]) {
                found = true;
                break;
            }
            j += 1;
        }
        if !found {
            return false;
        }
        i += 1;
    }
    true
}

/// `==` on `&str` is not `const`; this is.
const fn const_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// HTTP [`Transport`]: every discovered controller and self-mounted endpoint,
/// under the discovered transport-level wraps, routed by poem and served by the
/// transport's own accept loop.
///
/// [`from_config`](Self::from_config) is its one constructor, the call
/// [`HttpModule::for_root`](crate::HttpModule::for_root) makes:
///
/// ```
/// use nest_rs_http::{HttpConfig, HttpTransport};
///
/// let transport = HttpTransport::from_config(&HttpConfig {
///     compression: true,
///     ..Default::default()
/// })?;
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// Nothing configures it a second way — no `new`:
///
/// ```compile_fail
/// let transport = nest_rs_http::HttpTransport::new();
/// ```
///
/// and no `Default`:
///
/// ```compile_fail
/// let transport = nest_rs_http::HttpTransport::default();
/// ```
pub struct HttpTransport {
    bind: String,
    cors: Option<Cors>,
    tls: Option<HttpTls>,
    server_header: bool,
    global_prefix: Option<String>,
    max_body_bytes: usize,
    request_timeout: Option<Duration>,
    shutdown_timeout: Duration,
    max_concurrent_connections: usize,
    deadlines: crate::server::Deadlines,
    security_headers: crate::HttpSecurityHeaders,
    compression: bool,
    version_selector: Option<crate::versioning::VersionSelector>,
    /// What each self-mount runs off its connections, by mount path.
    detached: Vec<(String, DetachedWork)>,
    /// Shared with the edge, whose response bodies read it.
    drain: Arc<Drain>,
    endpoint: Option<BoxEndpoint<'static, Response>>,
}

fn normalize_global_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("/{trimmed}"))
}

/// The canonical form of a mount path: `"/x"`, with the root as `"/"`.
///
/// The collision checks compare mount paths as strings, while `Route::nest` makes
/// `"/x"` and `"/x/"` one key: left raw, the pair slips past and poem panics.
pub fn normalize_mount_path(raw: &str) -> String {
    match normalize_global_prefix(raw) {
        Some(path) => path,
        None => "/".to_owned(),
    }
}

/// `raw` as [`normalize_mount_path`] writes it, when every segment is a
/// literal the router matches as written: none empty, `.` or `..`, none
/// holding route-template syntax (`{name}`, a brace) or what the router would
/// read as a parameter (`:`, `*`, `<`), nor `%`, `?`, `#`, `\`, whitespace or
/// a control character. `None` otherwise.
pub fn literal_mount_path(raw: &str) -> Option<String> {
    let path = normalize_mount_path(raw);
    let literal = path.split('/').skip(1).all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment.contains(['{', '}', ':', '*', '<', '%', '?', '#', '\\'])
            && !segment.chars().any(|c| c.is_whitespace() || c.is_control())
    });
    (path == "/" || literal).then_some(path)
}

/// Claim the address `template` names for `owner`, or fail boot naming both
/// claimants — before poem panics in route assembly (`duplicate path:
/// <prefix>/*--poem-rest`). Keyed by the address, so two spellings of one
/// (`/x/{a}`, `/x/{b}`) are one claim.
fn claim_exclusive_path(
    owners: &mut HashMap<String, String>,
    kind: &str,
    template: &RouteTemplate,
    owner: String,
    remedy: &str,
) -> anyhow::Result<()> {
    if let Some(first) = owners.insert(template.identity().to_owned(), owner.clone()) {
        anyhow::bail!(
            "duplicate {kind} {:?}: {first} and {owner} both mount there — a {kind} is its \
             exclusive namespace; {remedy}",
            template.as_str(),
        );
    }
    Ok(())
}

/// A route mounted on an address, and the handler that declared it.
struct RouteClaim {
    template: String,
    controller: &'static str,
    handler: &'static str,
}

/// Claim `template`'s address for one controller's route, or fail boot naming
/// both handlers: poem mounts two spellings of one address apart, and one of
/// them never runs. Several verbs of one controller share an address.
fn claim_route(
    owners: &mut HashMap<String, RouteClaim>,
    template: &RouteTemplate,
    controller: &'static str,
    handler: &'static str,
) -> anyhow::Result<()> {
    match owners.entry(template.identity().to_owned()) {
        Entry::Vacant(slot) => {
            slot.insert(RouteClaim {
                template: template.as_str().to_owned(),
                controller,
                handler,
            });
        }
        Entry::Occupied(first) if first.get().controller == controller => {}
        Entry::Occupied(first) => {
            let first = first.get();
            anyhow::bail!(
                "duplicate route path: {}::{} mounts {:?} and {controller}::{handler} mounts \
                 {:?}, one address — an address is mounted once and each parameter binds by \
                 its name; give each controller route a distinct full path",
                first.controller,
                first.handler,
                first.template,
                template.as_str(),
            );
        }
    }
    Ok(())
}

/// Mount the router's fallback, or fail the boot when its path lies under a
/// prefix someone owns, where the router would answer everything first.
fn mount_fallback(
    container: &Container,
    meta: &HttpFallbackMeta,
    claims: Claims,
    get_routes: &[(String, &'static str)],
) -> anyhow::Result<Fallback> {
    RouteTemplate::literal(meta.path()).map_err(|refused| {
        anyhow::anyhow!("the router fallback {} answers at {refused}", meta.owner())
    })?;
    if let Some((prefix, owner)) = claims.owner_of(meta.path()) {
        anyhow::bail!(
            "the router fallback {} answers at {:?}, under {prefix:?}, which {owner} owns: \
             the route table claims everything there, so the fallback would answer nothing — \
             give it a path outside {prefix:?}",
            meta.owner(),
            meta.path(),
        );
    }
    if let Some((_, controller)) = get_routes.iter().find(|(path, _)| path == meta.path()) {
        tracing::warn!(
            target: crate::target::ROUTES,
            path = meta.path(),
            controller,
            fallback = meta.owner(),
            "a route answers GET at the router fallback's own path, which the fallback never \
             answers",
        );
    }
    tracing::info!(
        target: crate::target::ROUTES,
        kind = "fallback",
        path = meta.path(),
        "mounted endpoint",
    );
    Ok(Fallback::new(
        meta.path().to_owned(),
        claims,
        meta.mount(container).boxed(),
    ))
}

/// `path`, a controller's prefix or one of its routes, read as a template; the
/// macros read both halves, so a refusal here is a hand-built controller's.
fn template_of(path: &str, controller: &str) -> anyhow::Result<RouteTemplate> {
    RouteTemplate::parse(path).map_err(|refused| anyhow::anyhow!("{controller} mounts {refused}"))
}

/// What to do about two controllers claiming one mount prefix.
fn prefix_remedy(version: Option<&str>) -> String {
    match version {
        Some(version) => format!(
            "both declare version {version:?} at that path — drop it from one of the two \
             `#[controller(version = …)]` lists",
        ),
        None => "give each one a distinct path".to_owned(),
    }
}

impl HttpTransport {
    /// The transport `cfg` describes, each bounded field held to the range its
    /// variable is: a config built in code never met the environment.
    pub fn from_config(cfg: &crate::HttpConfig) -> anyhow::Result<Self> {
        let transport = Self {
            bind: format!("{}:{}", cfg.host, cfg.port),
            cors: cfg
                .cors
                .clone()
                .map(crate::HttpCors::into_middleware)
                .transpose()?,
            tls: cfg.tls.clone(),
            server_header: cfg.server_header,
            global_prefix: cfg
                .global_prefix
                .as_deref()
                .and_then(normalize_global_prefix),
            max_body_bytes: cfg.max_body_bytes.unwrap_or(crate::RawBody::DEFAULT_LIMIT),
            request_timeout: cfg.request_budget()?,
            shutdown_timeout: cfg.shutdown_window()?,
            max_concurrent_connections: cfg.connection_cap()?,
            deadlines: cfg.deadlines()?,
            security_headers: cfg.security_headers.clone(),
            compression: cfg.compression,
            // `None` is the URI strategy: the version is already in the mount path.
            version_selector: cfg.version_selector(),
            detached: Vec::new(),
            drain: Arc::default(),
            endpoint: None,
        };
        cfg.warn_when_requests_outlast_the_window();
        cfg.warn_when_the_body_gap_reaches_the_budget();
        Ok(transport)
    }

    /// Take the assembled endpoint for in-process testing (drive with poem's
    /// `TestClient`). Returns `None` before `configure` has run, and leaves
    /// the transport without an endpoint (so it must not also be `serve`d).
    pub fn take_endpoint(&mut self) -> Option<BoxEndpoint<'static, Response>> {
        self.endpoint.take()
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        let discovery = Discovery::new(container);
        // Before anything mounts: an unresolvable global layer spec would
        // otherwise be dropped silently.
        for d in discovery.meta::<HttpBootCheck>() {
            d.meta.run(container).map_err(|msg| anyhow::anyhow!(msg))?;
        }
        let mut route = Route::new();

        let global_guards = container.get::<GlobalGuardsActive>().is_some();
        let mut unguarded: Vec<String> = Vec::new();
        let mut prefix_owner: HashMap<String, String> = HashMap::new();
        // Routes mount flat (`<prefix>/<path>`), so distinct prefixes can still
        // collide on a full path.
        let mut route_owner: HashMap<String, RouteClaim> = HashMap::new();

        // Full routes, not prefixes, for the non-URI rewrite: a `/` prefix matches
        // nothing segment-wise, and `/posts` would claim another controller's `/posts/drafts`.
        let mut versioned_routes: Vec<String> = Vec::new();
        // Kept apart because they yield differently: a self-mount is neutral against
        // anything, an unversioned controller route only against a *default* version.
        let mut self_mounts: Vec<String> = Vec::new();
        let mut unversioned_routes: Vec<String> = Vec::new();
        // What the router's fallback never answers. A controller's prefix is its
        // namespace although its routes mount flat; one at `/` owns its routes alone.
        let mut claims = Claims::default();
        let mut get_routes: Vec<(String, &'static str)> = Vec::new();
        for d in discovery.meta::<HttpControllerMeta>() {
            let at_root = literal_prefix(d.meta.path) == "/";
            if !at_root {
                // The address a non-URI version selector is called at.
                claims.prefix(d.meta.path, d.meta.controller);
            }
            for version in d.meta.mounted_versions() {
                let prefix = d.meta.effective_prefix(version);
                if !at_root {
                    claims.prefix(&prefix, d.meta.controller);
                }
                claim_exclusive_path(
                    &mut prefix_owner,
                    "controller prefix",
                    &template_of(&prefix, d.meta.controller)?,
                    d.meta.controller.to_owned(),
                    &prefix_remedy(version),
                )?;
                for r in &d.meta.routes {
                    if !HttpControllerMeta::serves(r, version) {
                        continue;
                    }
                    let path = join_path(&prefix, r.path);
                    let template = template_of(&path, d.meta.controller)?;
                    // Under a non-URI selector a versioned route is called unversioned.
                    let addresses = match version {
                        Some(_) => vec![path.clone(), join_path(d.meta.path, r.path)],
                        None => vec![path.clone()],
                    };
                    for address in addresses {
                        if r.verb == crate::HttpVerb::Get {
                            get_routes.push((address.clone(), d.meta.controller));
                        }
                        if at_root {
                            claims.route(address);
                        }
                    }
                    match version.is_some() {
                        true => versioned_routes.push(path.clone()),
                        false => unversioned_routes.push(path.clone()),
                    }
                    claim_route(&mut route_owner, &template, d.meta.controller, r.handler)?;
                    // Under a non-URI strategy `/v{n}` is where the route mounts, not
                    // what a client calls, so the version is logged as its own field.
                    let (logged, version) = match &self.version_selector {
                        Some(_) => (join_path(d.meta.path, r.path), version),
                        None => (path.clone(), None),
                    };
                    tracing::info!(
                        target: crate::target::ROUTES,
                        controller = d.meta.controller,
                        method = r.verb.as_str(),
                        path = logged.as_str(),
                        version = version,
                        handler = r.handler,
                        "mounted route",
                    );
                    if r.access_is_implicit(global_guards) {
                        unguarded.push(format!("{} {} ({})", r.verb.as_str(), path, r.handler));
                    }
                }
            }
            route = d.meta.mount(container, route);
        }

        // Unchecked, an undeclared default would silently send every caller that
        // states no version to the unversioned route or a 404.
        if let Some(selector) = &self.version_selector
            && selector.rewrites()
            && let Some(default) = selector.default_version()
        {
            let declared = crate::declared_versions(container);
            if !declared.iter().any(|v| v == default) {
                let var = nest_rs_config::var_name("http", "DEFAULT_VERSION");
                anyhow::bail!(match declared.is_empty() {
                    true => format!(
                        "{var} names API version {default:?}, and no controller declares a \
                         version at all — declare it with #[controller(version = {default:?})] \
                         or unset {var}",
                    ),
                    false => format!(
                        "{var} names API version {default:?}, which no controller declares — \
                         the versions mounted are {}; name one of those or unset {var}",
                        declared.join(", "),
                    ),
                });
            }
        }

        if !unguarded.is_empty() {
            tracing::warn!(
                target: nest_rs_core::target::LAYERS,
                count = unguarded.len(),
                routes = unguarded.join(", ").as_str(),
                hint = "bind a guard or mark them #[public]",
                "unguarded routes detected",
            );
        }
        // Absent without a global guard pool. A `Guarded` self-mount has no route
        // shaper, so the transport runs the pool at its edge.
        let self_mount_guard = discovery
            .meta::<SelfMountGuardWrap>()
            .into_iter()
            .next()
            .map(|d| d.meta);
        // A warning, not a fail-secure stop: a gateway may bind its own
        // `#[use_guards]` inside its opaque mount closure.
        let mut unguarded_edges: Vec<String> = Vec::new();
        let mut endpoint_owner: HashMap<String, String> = HashMap::new();
        for d in discovery.meta::<HttpEndpointMeta>() {
            let owner = format!("{} endpoint {}", d.meta.label(), d.meta.owner());
            // Nested whole at its path; what it routes below is its own template.
            let mut templates = Vec::new();
            for (index, path) in d.meta.paths().enumerate() {
                let read = match index {
                    0 => RouteTemplate::literal(path),
                    _ => RouteTemplate::parse(path),
                };
                templates
                    .push(read.map_err(|refused| anyhow::anyhow!("the {owner} mounts {refused}"))?);
            }
            let identity = templates[0].identity();
            // A self-mount nests its whole subtree, so a controller on that path
            // is the same poem panic.
            if let Some(first) = prefix_owner
                .get(identity)
                .map(String::as_str)
                .or_else(|| route_owner.get(identity).map(|claim| claim.controller))
            {
                anyhow::bail!(
                    "duplicate mount path {:?}: controller {first} and {} endpoint {} both mount \
                     there — a mount path is its owner's exclusive namespace; give each one a \
                     distinct path",
                    d.meta.path(),
                    d.meta.label(),
                    d.meta.owner(),
                );
            }
            // Recorded unversioned so a versioned catch-all cannot swallow them;
            // exactly the declared paths — a surface owning a subtree says so via `also_mounts`.
            for template in &templates {
                self_mounts.push(template.as_str().to_owned());
                claims.prefix(template.as_str(), owner.clone());
                claim_exclusive_path(
                    &mut endpoint_owner,
                    "self-mounted endpoint path",
                    template,
                    owner.clone(),
                    "give each one a distinct path",
                )?;
            }
            tracing::info!(
                target: crate::target::ROUTES,
                kind = d.meta.label(),
                path = d.meta.path(),
                "mounted endpoint",
            );
            if let Some(work) = d.meta.detached() {
                self.detached.push((d.meta.path().to_owned(), work.clone()));
            }
            if d.meta.edge_access_is_implicit(global_guards) {
                unguarded_edges.push(format!("{} ({})", d.meta.path(), d.meta.label()));
            }
            match (d.meta.posture(), &self_mount_guard) {
                (EdgePosture::Guarded, Some(wrap)) => {
                    // `nest_no_strip`: the isolated sub-route matches its own full path.
                    let isolated: BoxEndpoint<'static, Response> =
                        d.meta.mount(container, Route::new()).boxed();
                    let wrapped = wrap.apply(container, isolated);
                    route = route.nest_no_strip(d.meta.path(), wrapped);
                }
                _ => {
                    // `Exempt`: gated in band (GraphQL, MCP) or public by design (OpenAPI).
                    route = d.meta.mount(container, route);
                }
            }
        }
        if !unguarded_edges.is_empty() {
            tracing::warn!(
                target: nest_rs_core::target::LAYERS,
                count = unguarded_edges.len(),
                endpoints = unguarded_edges.join(", ").as_str(),
                hint = "register a global guard pool or gate the gateway with #[use_guards]",
                "unguarded self-mount edges detected",
            );
        }
        // Inside the global prefix, so it rewrites the path controllers mount at.
        // `rewrites()`, not `Some`: wrapping the URI strategy would refuse every `/v{n}/…`.
        if let Some(selector) = self.version_selector.take().filter(|s| s.rewrites()) {
            let selector = selector.with_routes(
                versioned_routes,
                self_mounts,
                unversioned_routes,
                crate::declared_versions(container),
            );
            // An inert selector changes nothing and costs a routing layer (+57% on the hot path).
            if !selector.is_inert() {
                route = Route::new().nest_no_strip("/", VersionedEndpoint::new(route, selector));
            }
        }

        if let Some(prefix) = self.global_prefix.take() {
            let owner = format!(
                "the global prefix ({})",
                nest_rs_config::var_name("http", "GLOBAL_PREFIX"),
            );
            RouteTemplate::literal(&prefix)
                .map_err(|refused| anyhow::anyhow!("{owner} {refused}"))?;
            claims.only(prefix.clone(), owner);
            // Every route answers under the prefix, never at a path of its own.
            get_routes.clear();
            route = Route::new().nest(prefix, route);
        }
        let fallback = match discovery.meta::<HttpFallbackMeta>().as_slice() {
            [] => None,
            [only] => Some(mount_fallback(container, &only.meta, claims, &get_routes)?),
            [first, second, ..] => anyhow::bail!(
                "two router fallbacks: {} and {} — the router has one fallback, which answers \
                 whatever nothing else claims; import one of them",
                first.meta.owner(),
                second.meta.owner(),
            ),
        };
        let route = WithFallback::new(route, fallback);

        // Ascending priority, whatever the registration order; the stable sort
        // keeps insertion order within a band.
        let mut metas: Vec<std::sync::Arc<HttpEndpointWrap>> = discovery
            .meta::<HttpEndpointWrap>()
            .into_iter()
            .map(|d| d.meta)
            .collect();
        metas.sort_by_key(|m| m.priority());
        let mut edge_headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
        for (name, value) in self.security_headers.headers(self.tls.is_some()) {
            // Values are boot-validated: a failure here is a framework bug.
            match HeaderValue::from_str(&value) {
                Ok(header_value) => edge_headers.push((name, header_value)),
                Err(_) => tracing::error!(
                    target: crate::target::HTTP,
                    header = name.as_str(),
                    "failed to construct a security header despite boot validation",
                ),
            }
        }
        if self.server_header {
            edge_headers.push((SERVER, HeaderValue::from_static(SERVER_VALUE)));
        }
        // The problem normalizer must be outermost: without CORS or compression
        // that is the edge itself, otherwise a wrap outside both.
        let fuse_normalize = !self.compression && self.cors.is_none();
        let timeout = self.request_timeout.take();
        let body_limit = Some(self.max_body_bytes);

        let endpoint: BoxEndpoint<'static, Response> = if metas.is_empty() && fuse_normalize {
            crate::edge::EdgeEndpoint::new(
                route.map_to_response(),
                container.clone(),
                timeout,
                body_limit,
                self.deadlines.body_read,
                edge_headers,
                true,
                // No compression outside, so nothing can rewrite the body.
                false,
                Arc::clone(&self.drain),
            )
            .boxed()
        } else {
            // The router answers 404/405 with an `Err`, which would short-circuit an
            // interceptor's `next.run(req).await?`; rendering at `ERROR_RESOLVE` lets them see it.
            let split = metas
                .partition_point(|m| m.priority() < crate::interceptor::priority::ERROR_RESOLVE);
            let (below, above) = metas.split_at(split);
            let mut endpoint: BoxEndpoint<'static, Response> = if below.is_empty() {
                crate::problem::ResolvedErrors(route).boxed()
            } else {
                let mut inner: BoxEndpoint<'static, Response> = route.map_to_response().boxed();
                for meta in below {
                    inner = meta.wrap(container, inner);
                }
                crate::problem::ResolvedErrors(inner).boxed()
            };
            for meta in above {
                endpoint = meta.wrap(container, endpoint);
            }
            let mut endpoint: BoxEndpoint<'static, Response> = crate::edge::EdgeEndpoint::new(
                endpoint,
                container.clone(),
                timeout,
                body_limit,
                self.deadlines.body_read,
                edge_headers,
                fuse_normalize,
                // Compression replaces the request body but not its `Content-Length`.
                self.compression,
                Arc::clone(&self.drain),
            )
            .boxed();
            if self.compression {
                endpoint = endpoint.with(Compression::new()).map_to_response().boxed();
            }
            // Outside the edge, so a preflight carries no request scope.
            if let Some(cors) = self.cors.take() {
                endpoint = endpoint.with(cors).map_to_response().boxed();
            }
            if fuse_normalize {
                endpoint
            } else {
                endpoint
                    .around(|ep, req| async move {
                        Ok(match ep.call(req).await {
                            Ok(resp) => crate::problem::normalize_error_response(resp).await,
                            Err(err) => {
                                crate::problem::normalize_error_response(
                                    crate::problem::render_error(err),
                                )
                                .await
                            }
                        })
                    })
                    .map_to_response()
                    .boxed()
            }
        };

        self.endpoint = Some(endpoint);
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        #[expect(
            clippy::expect_used,
            reason = "the transport lifecycle runs configure before serve"
        )]
        let endpoint = self
            .endpoint
            .expect("HttpTransport::configure must run before serve");
        let window = self.shutdown_timeout;
        let drain = self.drain;
        // Checked before the bind: material that cannot serve fails the boot
        // with the port never taken.
        let tls = self
            .tls
            .map(HttpTls::into_listener)
            .transpose()
            .context("the configured TLS material cannot serve")?;
        let listener = tokio::net::TcpListener::bind(&self.bind)
            .await
            .with_context(|| format!("the HTTP transport cannot listen on {}", self.bind))?;
        tracing::info!(
            target: crate::target::HTTP,
            addr = %self.bind,
            tls = tls.is_some(),
            idle_timeout_secs = self.deadlines.idle.as_secs(),
            "transport listening",
        );
        let server = Server::new(
            listener,
            endpoint,
            Arc::clone(&drain),
            self.max_concurrent_connections,
            self.deadlines,
            tls,
        )?;
        let detached = self.detached;
        // `begin` runs before the loop stops accepting, so every socket the
        // window cuts drops after the instant it records.
        let signal = {
            let drain = Arc::clone(&drain);
            let leaving: Vec<DetachedWork> =
                detached.iter().map(|(_, work)| work.clone()).collect();
            async move {
                cancel.cancelled().await;
                drain.begin(window);
                for work in &leaving {
                    work.go_away();
                }
            }
        };
        server.run(signal).await;
        // Last, so nothing a connection carried still runs when the shutdown hooks start.
        DetachedWork::stop_at(&detached, drain.bound()).await;
        drain.report(window);
        Ok(())
    }

    /// The window, then the settle `serve` spends stopping what its
    /// self-mounts ran off their connections.
    fn stop_bound(&self) -> Duration {
        self.shutdown_timeout + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_path_concatenates_clean_segments() {
        assert_eq!(join_path("/health", "/live"), "/health/live");
        assert_eq!(join_path("/users", "/{id}"), "/users/{id}");
    }

    #[test]
    fn join_path_strips_redundant_slashes_on_either_side() {
        assert_eq!(join_path("/health/", "/live"), "/health/live");
        assert_eq!(join_path("/health", "live"), "/health/live");
        assert_eq!(join_path("/health/", "live"), "/health/live");
    }

    #[test]
    fn join_path_handles_empty_or_root_segments() {
        assert_eq!(join_path("", ""), "/");
        assert_eq!(join_path("/", ""), "/");
        assert_eq!(join_path("/", "/"), "/");
        assert_eq!(join_path("", "/users"), "/users");
        assert_eq!(join_path("/users", ""), "/users");
    }

    #[test]
    fn version_path_prefixes_when_a_version_is_supplied() {
        assert_eq!(version_path(Some("1"), "/users"), "/v1/users");
        assert_eq!(version_path(Some("2"), "/users/{id}"), "/v2/users/{id}");
        assert_eq!(version_path(Some("1"), "/"), "/v1");
    }

    #[test]
    fn version_path_leaves_an_unversioned_path_alone() {
        assert_eq!(version_path(None, "/users"), "/users");
        assert_eq!(version_path(None, "/"), "/");
    }

    #[test]
    fn a_literal_mount_path_is_kept_canonical() {
        assert_eq!(literal_mount_path("").as_deref(), Some("/"));
        assert_eq!(literal_mount_path("assets/").as_deref(), Some("/assets"));
        assert_eq!(
            literal_mount_path("/.well-known").as_deref(),
            Some("/.well-known")
        );
        assert_eq!(literal_mount_path("/app/v2").as_deref(), Some("/app/v2"));
    }

    #[test]
    fn a_mount_path_the_router_would_read_as_a_template_is_refused() {
        for raw in [
            "/assets/*rest",
            "/:id",
            "/{id}",
            "/assets/{*rest}",
            "/a{{b",
            "/files/<\\d+>",
            "/a//b",
            "/a/../b",
            "/a%2fb",
            "/a b",
            "/a?b",
            "/a\\b",
        ] {
            assert_eq!(literal_mount_path(raw), None, "{raw}");
        }
    }

    /// The transport a default config describes.
    fn defaults() -> HttpTransport {
        HttpTransport::from_config(&crate::HttpConfig::default())
            .expect("the defaults are in range")
    }

    #[test]
    fn a_default_config_builds_a_plaintext_transport_on_every_interface() {
        let d = defaults();
        assert_eq!(d.bind, "0.0.0.0:3000");
        assert!(d.cors.is_none());
        assert!(d.tls.is_none());
        assert!(!d.server_header, "no framework fingerprint by default");
        assert!(d.global_prefix.is_none());
        assert_eq!(d.max_body_bytes, crate::RawBody::DEFAULT_LIMIT);
        assert!(
            d.version_selector.is_none(),
            "the URI strategy routes by path"
        );
        assert!(d.endpoint.is_none());
        assert_eq!(
            d.shutdown_timeout,
            crate::HttpConfig::default().shutdown_timeout
        );
    }

    #[test]
    fn a_built_config_takes_its_connection_cap_and_is_held_to_its_range() {
        let capped = HttpTransport::from_config(&crate::HttpConfig {
            max_concurrent_connections: 64,
            ..Default::default()
        })
        .expect("a cap inside the range");
        assert_eq!(capped.max_concurrent_connections, 64);
        for cap in [0, 1_048_577] {
            let refused = HttpTransport::from_config(&crate::HttpConfig {
                max_concurrent_connections: cap,
                ..Default::default()
            })
            .err()
            .expect("a cap outside the range never reaches the loop")
            .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name(
                    "http",
                    "MAX_CONCURRENT_CONNECTIONS"
                )),
                "{cap}: {refused}"
            );
        }
    }

    /// The one constructor holds the window and the budget to the ranges their
    /// variables are, for a config that never met the environment.
    #[test]
    fn a_built_config_holds_its_shutdown_window_and_request_budget_to_their_ranges() {
        for (config, key, field) in [
            (
                crate::HttpConfig {
                    shutdown_timeout: Duration::MAX,
                    ..Default::default()
                },
                "SHUTDOWN_TIMEOUT_SECS",
                "HttpConfig::shutdown_timeout",
            ),
            (
                crate::HttpConfig {
                    shutdown_timeout: Duration::ZERO,
                    ..Default::default()
                },
                "SHUTDOWN_TIMEOUT_SECS",
                "HttpConfig::shutdown_timeout",
            ),
            (
                crate::HttpConfig {
                    request_timeout: Some(Duration::ZERO),
                    ..Default::default()
                },
                "REQUEST_TIMEOUT_SECS",
                "HttpConfig::request_timeout",
            ),
            (
                crate::HttpConfig {
                    request_timeout: Some(Duration::MAX),
                    ..Default::default()
                },
                "REQUEST_TIMEOUT_SECS",
                "HttpConfig::request_timeout",
            ),
        ] {
            let refused = HttpTransport::from_config(&config)
                .err()
                .expect("a value outside the range never reaches the transport")
                .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("http", key)),
                "{field}: {refused}"
            );
            assert!(refused.contains(field), "{field}: {refused}");
        }
        let off = HttpTransport::from_config(&crate::HttpConfig {
            request_timeout: None,
            ..Default::default()
        })
        .expect("`None` turns the budget off");
        assert_eq!(off.request_timeout, None);
    }

    #[test]
    fn the_stop_bound_is_the_configured_window_then_the_settle() {
        let window = Duration::from_secs(3);
        let t = HttpTransport::from_config(&crate::HttpConfig {
            shutdown_timeout: window,
            ..Default::default()
        })
        .expect("a window in range");
        assert_eq!(
            t.stop_bound(),
            window + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT
        );
    }

    #[test]
    fn the_listen_address_is_the_configured_host_and_port() {
        let t = HttpTransport::from_config(&crate::HttpConfig {
            host: "127.0.0.1".into(),
            port: 9000,
            ..Default::default()
        })
        .expect("in range");
        assert_eq!(t.bind, "127.0.0.1:9000");
    }

    #[test]
    fn configured_tls_material_is_kept_for_serve_to_check() {
        let t = HttpTransport::from_config(&crate::HttpConfig {
            tls: Some(HttpTls::new(b"cert".to_vec(), b"key".to_vec())),
            ..Default::default()
        })
        .expect("material is checked when serve binds, not here");
        assert!(t.tls.is_some());
    }

    #[test]
    fn the_server_header_names_this_nestrs_version() {
        let t = HttpTransport::from_config(&crate::HttpConfig {
            server_header: true,
            ..Default::default()
        })
        .expect("in range");
        assert!(t.server_header);
        assert_eq!(
            SERVER_VALUE,
            format!("nestrs/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn a_global_prefix_is_kept_in_its_canonical_form() {
        for (raw, kept) in [
            ("api", Some("/api")),
            ("/api/", Some("/api")),
            ("/", None),
            ("  ", None),
        ] {
            let t = HttpTransport::from_config(&crate::HttpConfig {
                global_prefix: Some(raw.into()),
                ..Default::default()
            })
            .expect("in range");
            assert_eq!(t.global_prefix.as_deref(), kept, "{raw:?}");
        }
    }

    #[test]
    fn take_endpoint_returns_none_before_configure_has_run() {
        let mut t = defaults();
        assert!(t.take_endpoint().is_none(), "no endpoint before configure");
    }
}

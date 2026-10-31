use std::net::IpAddr;
use std::time::Duration;

use http::HeaderName;
use nest_rs_config::{
    Bound, ConfigService, CountBounds, DurationBounds, Field, Floor, Namespaced, Result, config,
};

use crate::cors::HttpCors;
use crate::raw_body::RawBody;
use crate::security_headers::HttpSecurityHeaders;
use crate::tls::HttpTls;
use crate::versioning::{ApiVersioning, DEFAULT_VERSION_HEADER, VersionSelector};

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 3000;
/// 20 seconds, plus 5 for the shutdown hooks, 3 for the telemetry flush and 1
/// for the console's (`nest_rs_core::SHUTDOWN_HOOKS_TIMEOUT`): 29, under
/// Kubernetes' default 30.
pub(crate) const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

const SHUTDOWN_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a shorter window cuts every request in flight at the signal, and `0` is not the \
         off switch it is for `REQUEST_TIMEOUT_SECS`: a shutdown that waits without a bound \
         is what this window exists to prevent",
    )),
    Bound::new(
        3600,
        "a window past an hour is a unit slip more often than a choice (`25000` meant as \
         milliseconds is seven hours), and it outlasts the grace period an orchestrator \
         gives a replica, whose kill then cuts what the window held without a word",
    ),
);

/// The lifetime ceiling every long-lived connection takes by default: four
/// hours, for an `#[sse]` stream here, a WebSocket and a GraphQL subscription
/// in their own crates.
///
/// Each edge reads its own variable — `<PREFIX>_HTTP__SSE_MAX_CONNECTION_SECS`,
/// `<PREFIX>_WS__MAX_CONNECTION_SECS`, `<PREFIX>_GRAPHQL__MAX_CONNECTION_SECS` —
/// since each runs its own loop and may want its own ceiling: a deployment
/// tightening how long a revoked credential keeps a live socket sets all three.
pub const DEFAULT_MAX_CONNECTION: Duration = Duration::from_secs(4 * 60 * 60);

/// The interval a server-sent stream keeps itself alive at by default — HTTP's
/// `#[sse]` here, MCP's stream in its own crate: 15 seconds, under the 30–60 s
/// idle timeout common to proxies and browsers. Each reads its own variable.
pub const DEFAULT_SSE_KEEP_ALIVE: Duration = Duration::from_secs(15);

/// The deadline an edge's unit takes by default — HTTP's request here, a
/// WebSocket message's and hooks' and an MCP operation's in their own crates:
/// the shutdown window's own default, so a unit admitted just before a stop is
/// answered inside the window, and above every net it waits under. Each reads
/// its own variable.
pub const DEFAULT_UNIT_TIMEOUT: Duration = DEFAULT_SHUTDOWN_TIMEOUT;

/// The range every long-lived connection's lifetime ceiling is held to — an
/// `#[sse]` stream here, a WebSocket and a GraphQL subscription in their own
/// crates — from a second to a day; each is an `Option<Duration>`, so `0` from
/// the environment and `None` in code turn it off.
pub const MAX_CONNECTION_BOUNDS: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a ceiling under a second ends every connection as it opens, so its client \
         reconnects in a loop",
    )),
    Bound::new(
        24 * 60 * 60,
        "a connection keeps the privileges it authenticated with until it ends, so past a \
         day it outlives the token that admitted it — an hour by default — many times over; \
         a deployment that means no ceiling turns it off",
    ),
);

/// The range of a server-sent stream's keep-alive interval — HTTP's `#[sse]`
/// here, MCP's stream in its own crate — from a second to an hour, `0` turning
/// it off.
pub const SSE_KEEP_ALIVE_BOUNDS: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a comment under a second apart is traffic, not a keep-alive",
    )),
    Bound::new(
        60 * 60,
        "a keep-alive exists to beat an intermediary's idle timeout, and those are minutes — \
         nginx's and a cloud load balancer's sixty seconds by default — so one sent less \
         often than hourly keeps nothing alive",
    ),
);

/// The range of an edge's per-unit timeout — HTTP's request here, a WebSocket
/// message's and an MCP operation's in their own crates — from a second to an
/// hour, `0` turning it off.
pub const UNIT_TIMEOUT_BOUNDS: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a timeout under a second cuts every unit before its handler has run",
    )),
    Bound::new(
        60 * 60,
        "a unit still running after an hour has outlived every client and proxy timeout in \
         front of it, and a timeout that long is a typo more often than a choice (`30000` \
         meant as milliseconds is eight hours); a handler that runs longer turns it off",
    ),
);

/// hyper's own head default, and Kestrel's `RequestHeadersTimeout`.
pub(crate) const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Under the default request timeout: that budget starts at the request's
/// arrival, before any wait on its body, so a gap as long could never fire
/// first, and a body that stops arriving would be answered the budget's `503`
/// rather than `408`. Counted between two reads, as nginx's
/// `client_body_timeout` is.
pub(crate) const DEFAULT_BODY_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// nginx's `keepalive_timeout`, above the 60 s idle of AWS's load balancer and
/// of nginx's upstream connections, so the balancer closes first.
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(75);

/// nginx's `send_timeout`, counted between two writes.
pub(crate) const DEFAULT_SEND_TIMEOUT: Duration = Duration::from_secs(60);

const HEADER_READ_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a shorter deadline cuts a client on a slow network before its request is out, and \
         `0` is no off switch: a head without a deadline is how a client that sends nothing \
         holds a connection",
    )),
    Bound::new(
        300,
        "a head is a few hundred bytes, so a client taking five minutes over one is holding \
         the connection rather than sending it, and a deadline that long no longer bounds \
         what a slow head holds",
    ),
);

const BODY_READ_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a shorter bound fails an upload on a slow network between two of its packets, and \
         `0` is no off switch: a body that stops arriving would hold its request, its task \
         and its place under the connection cap for as long as the client likes",
    )),
    Bound::new(
        3600,
        "a body that has sent nothing for an hour has stopped, and a bound that long is a \
         unit slip more often than a choice (`30000` meant as milliseconds is eight hours)",
    ),
);

const IDLE_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a shorter deadline closes a kept-alive connection between two requests of one \
         client, which then pays a new handshake for each, and `0` is no off switch: an idle \
         connection holds its place under the connection cap",
    )),
    Bound::new(
        3600,
        "a connection idle for an hour is one its client forgot, holding a place under the \
         connection cap, and a deadline that long is a unit slip more often than a choice \
         (`75000` meant as milliseconds is twenty hours)",
    ),
);

const SEND_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a shorter deadline cuts a reader on a slow network mid-body, and `0` is no off \
         switch: a peer that stops reading would hold the write, its buffers and its place \
         under the connection cap forever",
    )),
    Bound::new(
        3600,
        "a peer that has taken no byte for an hour has stopped reading, and a deadline that \
         long is a unit slip more often than a choice (`60000` meant as milliseconds is \
         seventeen hours)",
    ),
);

pub(crate) const DEFAULT_MAX_CONCURRENT_CONNECTIONS: usize = 10_000;

/// [`HttpConfig::version_header`], for a config that never met the
/// environment.
const VERSION_HEADER: Field = Field::new("VERSION_HEADER", "HttpConfig::version_header");

const CONNECTIONS: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a cap of none binds the port and never accepts a connection",
    ),
    Bound::new(
        1 << 20,
        "a process holds no more open files than Linux's `fs.nr_open` allows, 1048576 by \
         default, so the kernel refuses a socket past it before the cap is reached",
    ),
);

/// HTTP transport options resolved at boot.
///
/// `<PREFIX>_HTTP__*` overlays the struct pinned in
/// [`HttpModule::for_root`](crate::HttpModule::for_root) per field; see
/// [`Config`](nest_rs_config::Config) for the precedence chain.
#[config(namespace = "http")]
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// Bind address; defaults to `0.0.0.0` (all interfaces).
    pub host: String,
    /// Listen port; defaults to `3000`.
    pub port: u16,
    /// PEM cert + key for HTTPS. `None` ⇒ plain HTTP. Picked up from
    /// `<PREFIX>_HTTP__TLS_CERT[_FILE]` + `<PREFIX>_HTTP__TLS_KEY[_FILE]`.
    #[config(with = tls)]
    pub tls: Option<HttpTls>,
    /// CORS policy. `None` ⇒ no CORS layer. Populated when
    /// `<PREFIX>_HTTP__CORS_ORIGINS` is set (see [`HttpCors`]). Its origins are
    /// also the browser origins a WebSocket gateway, the graphql-ws upgrade and
    /// `/mcp` admit; with `None`, those admit the request's own origin alone.
    #[config(with = cors)]
    pub cors: Option<HttpCors>,
    /// How a caller selects an API version: `uri` (the default), `header` or
    /// `media_type`. Read from `<PREFIX>_HTTP__VERSIONING`.
    pub versioning: ApiVersioning,
    /// The header [`ApiVersioning::Header`] reads. Defaults to
    /// `X-API-Version`; read from `<PREFIX>_HTTP__VERSION_HEADER`. Ignored by the
    /// other two strategies, but a value that is no HTTP header name fails the
    /// boot under any, from the environment and from the pinned struct alike.
    #[config(with = version_header)]
    pub version_header: String,
    /// The version served to a caller that states none. `None` (the default)
    /// leaves such a request on the unversioned routes. Read from
    /// `<PREFIX>_HTTP__DEFAULT_VERSION`.
    pub default_version: Option<String>,
    /// `true` ⇒ emit `Server: nestrs/<version>` on every response that sets
    /// none. Defaults to `false`: no framework fingerprint.
    pub server_header: bool,
    /// Mount every controller under a shared path prefix (e.g. `/api`). `None`
    /// ⇒ no prefix. Read from `<PREFIX>_HTTP__GLOBAL_PREFIX`; a missing leading
    /// `/` is added and a trailing one stripped, and empty or `"/"` is no
    /// prefix. One literal path: a template's `{` `}`, or a `:`, `*` or `<` the
    /// router reads as a parameter, fails the boot.
    #[config(with = global_prefix)]
    pub global_prefix: Option<String>,
    /// Transport-wide cap on the request body size, for every reader — an
    /// extractor, a guard, a global pipe — counted as the body is read, after
    /// routing and the guards: the edge reads nothing, and refuses only a
    /// declared `Content-Length` past it. A per-route
    /// [`RawBody::extract_with_limit`](crate::RawBody::extract_with_limit) can
    /// pin a tighter one. Defaults to 2 MiB ([`RawBody::DEFAULT_LIMIT`]); there
    /// is no off. Read from `<PREFIX>_HTTP__MAX_BODY_BYTES`; `0` fails the boot.
    #[validate(range(min = 1, message = "must be at least 1 byte"))]
    pub max_body_bytes: usize,
    /// Wall-clock budget for a single request, counted from its arrival, so the
    /// reading of its body counts against it; a request still running past it
    /// is aborted with `503 Service Unavailable` and a `Retry-After`. `None` ⇒
    /// no timeout. Read from `<PREFIX>_HTTP__REQUEST_TIMEOUT_SECS`, whole
    /// seconds from 1 to 3600 or `0` for no timeout — refused outside, from the
    /// environment and from the pinned struct alike; defaults to 20 seconds,
    /// the [`shutdown_timeout`](Self::shutdown_timeout) default, so a request
    /// admitted just before a stop is answered inside the window. A budget past
    /// the window, or none, is said once at boot with a `warn`. Off in code is
    /// `None`; a pinned zero is refused.
    #[config(bounds = UNIT_TIMEOUT_BOUNDS)]
    pub request_timeout: Option<Duration>,
    /// Default security response headers (`nosniff`, `X-Frame-Options`,
    /// `Referrer-Policy`, the `Cross-Origin-*` pair, HSTS under TLS), each on a
    /// response that sets none. On by default; tune via
    /// `<PREFIX>_HTTP__SECURITY_HEADERS_ENABLED` (the switch) and one key per
    /// header, each its field's name after `SECURITY_HEADERS_` — see
    /// [`HttpSecurityHeaders`].
    #[config(with = security_headers)]
    pub security_headers: HttpSecurityHeaders,
    /// Negotiate response compression (gzip / deflate / brotli / zstd) from the
    /// request's `Accept-Encoding`. Off by default; read from
    /// `<PREFIX>_HTTP__COMPRESSION`.
    pub compression: bool,
    /// Emit one access event per request on `nest_rs::operation` — method, path,
    /// status, byte-exact size, duration, client, `trace_id` and `span_id`, plus
    /// `actor_id` once an authn guard resolved a principal. **On by default**,
    /// and read from `<PREFIX>_HTTP__ACCESS_LOG`. Turning it off leaves the
    /// correlation id minted and echoed.
    pub access_log: bool,
    /// Reverse proxies whose `Forwarded` / `X-Forwarded-For` / `X-Real-IP` this
    /// deployment believes. Empty by default: every caller is identified by its
    /// transport peer.
    ///
    /// Read from `<PREFIX>_HTTP__TRUSTED_PROXIES` (`10.0.0.1,10.0.0.2`) by
    /// [`ClientIp`](crate::ClientIp), the throttler's rate-limit bucket and the
    /// browser-origin check of the WebSocket gateways, the graphql-ws upgrade
    /// and `/mcp`, where a trusted proxy's `Forwarded` (`proto=`, `host=`) or
    /// `X-Forwarded-Proto` / `X-Forwarded-Host` set the request's own origin;
    /// an unparseable entry aborts the boot. See
    /// [`ClientOrigin`](crate::ClientOrigin) for the resolution rule.
    pub trusted_proxies: Vec<IpAddr>,
    /// Maximum lifetime of a single `#[sse]` stream: when it elapses the stream
    /// ends, so the client's `EventSource` reconnects through the guard chain.
    /// A peer that stops reading is cut sooner, at
    /// [`send_timeout`](Self::send_timeout).
    ///
    /// A security control: 4 hours by default, one second to a day
    /// ([`MAX_CONNECTION_BOUNDS`]), `0` ⇒ unlimited. Read from
    /// `<PREFIX>_HTTP__SSE_MAX_CONNECTION_SECS`.
    #[config(bounds = MAX_CONNECTION_BOUNDS)]
    pub sse_max_connection: Option<Duration>,
    /// How often a `#[sse]` stream emits a keep-alive comment, so an idle
    /// stream is not dropped by an intermediary that sees no bytes. `None` ⇒
    /// none sent. Read from `<PREFIX>_HTTP__SSE_KEEP_ALIVE_SECS`, whole seconds
    /// from 1 to 3600 or `0` for none; defaults to 15 seconds.
    #[config(bounds = SSE_KEEP_ALIVE_BOUNDS)]
    pub sse_keep_alive: Option<Duration>,
    /// How long the transport lets open connections finish after a shutdown
    /// signal; what is still open then is closed, and one `warn` on
    /// `nest_rs::http` counts it. An `#[sse]` or MCP `GET` stream ends at the
    /// signal; a WebSocket or graphql-ws socket closes `1001 Going Away` after
    /// what it is answering. A socket a hand-built self-mount upgraded is
    /// never waited for: it ends with its handler or the process, counted as
    /// `upgraded_open`.
    ///
    /// Read from `<PREFIX>_HTTP__SHUTDOWN_TIMEOUT_SECS`, whole seconds from 1 to
    /// 3600 — refused outside, from the environment and from the pinned struct
    /// alike; defaults to 20 seconds. Keep the pod's grace period above it plus
    /// 9.5 seconds (`serve`'s unwind, the shutdown hooks, the telemetry flush and
    /// the console's).
    #[config(bounds = SHUTDOWN_TIMEOUT)]
    pub shutdown_timeout: Duration,
    /// How many connections the transport holds open at once, an upgraded
    /// socket counted until it closes. Past it a new connection waits in the
    /// kernel's listen backlog — it sees latency, then the kernel's own refusal
    /// once the backlog is full, never a reset of a connection it holds — and
    /// one `warn` on `nest_rs::http` names the variable.
    ///
    /// Read from `<PREFIX>_HTTP__MAX_CONCURRENT_CONNECTIONS`, from 1 to
    /// 1 048 576 — refused outside, from the environment and from the pinned
    /// struct alike; defaults to 10 000.
    #[config(bounds = CONNECTIONS)]
    pub max_concurrent_connections: usize,
    /// How long a connection has to deliver its first request head, counted
    /// from accept with the TLS handshake and HTTP/2's preface inside it, and on
    /// HTTP/1 how long a later head has from its first byte. Past it the
    /// connection is closed, so a client sending nothing, or a head a byte at a
    /// time, frees its place under the connection cap.
    ///
    /// Read from `<PREFIX>_HTTP__HEADER_READ_TIMEOUT_SECS`, whole seconds from 1
    /// to 300 — refused outside, from the environment and from the pinned struct
    /// alike; defaults to 30 seconds.
    #[config(bounds = HEADER_READ_TIMEOUT)]
    pub header_read_timeout: Duration,
    /// How long a kept-alive connection waits for its next request once its last
    /// answer has ended: then an HTTP/1 connection is closed and an HTTP/2 one
    /// is sent `GOAWAY`. A load balancer keeping its connections to this server
    /// open longer reuses one as it closes and answers that request `502`: set
    /// it above the balancer's own idle timeout.
    ///
    /// Read from `<PREFIX>_HTTP__IDLE_TIMEOUT_SECS`, whole seconds from 1 to 3600
    /// — refused outside, from the environment and from the pinned struct alike;
    /// defaults to 75 seconds.
    #[config(bounds = IDLE_TIMEOUT)]
    pub idle_timeout: Duration,
    /// How long a write waits on a peer that takes nothing: past it the
    /// connection is dropped — a response body, an event stream or a WebSocket
    /// alike. Counted between two writes, so a slow reader that keeps taking
    /// bytes is never cut.
    ///
    /// Read from `<PREFIX>_HTTP__SEND_TIMEOUT_SECS`, whole seconds from 1 to 3600
    /// — refused outside, from the environment and from the pinned struct alike;
    /// defaults to 60 seconds.
    #[config(bounds = SEND_TIMEOUT)]
    pub send_timeout: Duration,
    /// How long a request body may go without a byte once something reads it.
    /// Past it the read fails and the request is answered `408 Request
    /// Timeout` — on HTTP/1 with `Connection: close`, since the unread rest
    /// leaves the connection unusable; on HTTP/2 its stream alone ends.
    /// Counted between two frames, so an upload that keeps sending is never
    /// cut by it; the whole request is
    /// [`request_timeout`](Self::request_timeout)'s, the one bound on a body
    /// when that is off. Kept under it, since that budget counts from the
    /// request's arrival: at or past it, a body that stops arriving is answered
    /// the budget's `503` first, which is said once at boot with a `warn`.
    ///
    /// Read from `<PREFIX>_HTTP__BODY_READ_TIMEOUT_SECS`, whole seconds from 1
    /// to 3600 — refused outside, from the environment and from the pinned
    /// struct alike; defaults to 10 seconds.
    #[config(bounds = BODY_READ_TIMEOUT)]
    pub body_read_timeout: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            tls: None,
            cors: None,
            versioning: ApiVersioning::default(),
            version_header: DEFAULT_VERSION_HEADER.to_string(),
            default_version: None,
            server_header: false,
            global_prefix: None,
            max_body_bytes: RawBody::DEFAULT_LIMIT,
            request_timeout: Some(DEFAULT_UNIT_TIMEOUT),
            security_headers: HttpSecurityHeaders::default(),
            compression: false,
            access_log: true,
            trusted_proxies: Vec::new(),
            sse_max_connection: Some(DEFAULT_MAX_CONNECTION),
            sse_keep_alive: Some(DEFAULT_SSE_KEEP_ALIVE),
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            max_concurrent_connections: DEFAULT_MAX_CONCURRENT_CONNECTIONS,
            header_read_timeout: DEFAULT_HEADER_READ_TIMEOUT,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            send_timeout: DEFAULT_SEND_TIMEOUT,
            body_read_timeout: DEFAULT_BODY_READ_TIMEOUT,
        }
    }
}

impl HttpConfig {
    /// The version selector this configuration describes, or `None` under the
    /// URI strategy, which routing resolves;
    /// [`version_header`](Self::version_header) is refused as its variable is,
    /// whatever the strategy, for a config that never met the environment.
    pub(crate) fn version_selector(&self) -> Result<Option<VersionSelector>> {
        let header = header_name(
            nest_rs_config::var_name(
                <Self as nest_rs_config::Namespaced>::NAMESPACE,
                VERSION_HEADER.key(),
            ),
            VERSION_HEADER,
            &self.version_header,
            "",
        )?;
        let selector = VersionSelector::new(self.versioning, header, self.default_version.clone());
        Ok(selector.rewrites().then_some(selector))
    }

    /// [`max_concurrent_connections`](Self::max_concurrent_connections), held to
    /// the range the variable is, for a config that never met the environment.
    pub(crate) fn connection_cap(&self) -> Result<usize> {
        Self::MAX_CONCURRENT_CONNECTIONS.check(Self::NAMESPACE, self.max_concurrent_connections)
    }

    /// [`request_timeout`](Self::request_timeout), held to the range its
    /// variable is, for a config that never met the environment.
    pub(crate) fn request_budget(&self) -> Result<Option<Duration>> {
        Self::REQUEST_TIMEOUT.check(Self::NAMESPACE, self.request_timeout)
    }

    /// [`shutdown_timeout`](Self::shutdown_timeout), held to the range its
    /// variable is, for a config that never met the environment.
    pub(crate) fn shutdown_window(&self) -> Result<Duration> {
        Self::SHUTDOWN_TIMEOUT.check(Self::NAMESPACE, self.shutdown_timeout)
    }

    /// The four connection deadlines, each held to the range its variable is,
    /// for a config that never met the environment.
    pub(crate) fn deadlines(&self) -> Result<crate::server::Deadlines> {
        Ok(crate::server::Deadlines {
            header_read: Self::HEADER_READ_TIMEOUT
                .check(Self::NAMESPACE, self.header_read_timeout)?,
            body_read: Self::BODY_READ_TIMEOUT.check(Self::NAMESPACE, self.body_read_timeout)?,
            idle: Self::IDLE_TIMEOUT.check(Self::NAMESPACE, self.idle_timeout)?,
            send: Self::SEND_TIMEOUT.check(Self::NAMESPACE, self.send_timeout)?,
        })
    }

    /// One boot `warn` when [`request_timeout`](Self::request_timeout) is off
    /// or outlasts [`shutdown_timeout`](Self::shutdown_timeout): a request
    /// admitted just before a stop can then be cut unanswered. Not a refusal —
    /// a deployment may accept it knowingly.
    pub(crate) fn warn_when_requests_outlast_the_window(&self) {
        if self
            .request_timeout
            .is_some_and(|budget| budget <= self.shutdown_timeout)
        {
            return;
        }
        tracing::warn!(
            target: crate::target::HTTP,
            request_timeout_secs = self.request_timeout.map_or(0, |budget| budget.as_secs()),
            shutdown_timeout_secs = self.shutdown_timeout.as_secs(),
            "the request timeout outlasts the shutdown window: a request admitted just before a \
             stop can be cut unanswered",
        );
    }

    /// One boot `warn` when [`body_read_timeout`](Self::body_read_timeout)
    /// reaches [`request_timeout`](Self::request_timeout): the budget counts
    /// from the request's arrival, so the gap can never fire first and a body
    /// that stops arriving is answered the budget's `503`, never `408`. Not a
    /// refusal — the request is still bounded.
    pub(crate) fn warn_when_the_body_gap_reaches_the_budget(&self) {
        let Some(budget) = self.request_timeout else {
            return;
        };
        if self.body_read_timeout < budget {
            return;
        }
        tracing::warn!(
            target: crate::target::HTTP,
            body_read_timeout_secs = self.body_read_timeout.as_secs(),
            request_timeout_secs = budget.as_secs(),
            "the body-read timeout reaches the request timeout: a body that stops arriving is \
             answered the request timeout's 503, never 408",
        );
    }

    /// `<PREFIX>_HTTP__MAX_CONCURRENT_CONNECTIONS`, as this process spells it.
    pub(crate) fn connection_cap_variable() -> String {
        Self::MAX_CONCURRENT_CONNECTIONS.var(Self::NAMESPACE)
    }
}

/// The server's own certificate, its vocabulary's `TLS_*` keys; a reading
/// failure that names no variable is named under them, its I/O source kept.
fn tls(env: &ConfigService, field: Field, base: Option<HttpTls>) -> Result<Option<HttpTls>> {
    debug_assert_eq!(
        field.key(),
        "TLS",
        "HttpTls reads its fields' names under `tls`"
    );
    HttpTls::from_env(env, base).map_err(|e| match e.downcast::<nest_rs_config::ConfigError>() {
        Ok(worded) => worded,
        Err(e) => nest_rs_config::ConfigError::parse(env.var_name("TLS_*"), format!("{e:#}")),
    })
}

/// The CORS policy, each key its field's name after `CORS_`.
fn cors(env: &ConfigService, field: Field, base: Option<HttpCors>) -> Result<Option<HttpCors>> {
    debug_assert_eq!(
        field.key(),
        "CORS",
        "HttpCors reads its fields' names under `cors`"
    );
    HttpCors::from_env(env, base)
}

/// The security headers, each key its field's name after `SECURITY_HEADERS_`;
/// a header's 6.x flat key is refused naming the one it became.
fn security_headers(
    env: &ConfigService,
    field: Field,
    base: HttpSecurityHeaders,
) -> Result<HttpSecurityHeaders> {
    debug_assert_eq!(
        field.key(),
        "SECURITY_HEADERS",
        "HttpSecurityHeaders reads its fields' names under `security_headers`"
    );
    refuse_retired_header_keys(env)?;
    HttpSecurityHeaders::from_env(env, base)
}

/// Each security header's 6.x key, a flat one beside `SECURITY_HEADERS`.
const RETIRED_HEADER_KEYS: [&str; 9] = [
    "HSTS",
    "FRAME_OPTIONS",
    "CONTENT_TYPE_OPTIONS",
    "REFERRER_POLICY",
    "CROSS_ORIGIN_OPENER_POLICY",
    "CROSS_ORIGIN_RESOURCE_POLICY",
    "CROSS_ORIGIN_EMBEDDER_POLICY",
    "PERMISSIONS_POLICY",
    "CONTENT_SECURITY_POLICY",
];

/// Refuse a security header set under its 6.x key: read by nothing, it would
/// leave the header to its default — a policy the deployment wrote not sent at
/// all — where an unread variable's `warn` is all a deployment would hear. A
/// detector of the old spelling, never an alias of it.
fn refuse_retired_header_keys(env: &ConfigService) -> Result<()> {
    for retired in RETIRED_HEADER_KEYS {
        if let Some(setting) = env.setting(retired)? {
            return Err(nest_rs_config::ConfigError::Retired {
                var: setting.var().to_owned(),
                read_as: env.var_name(&format!("SECURITY_HEADERS_{retired}")),
                why: "it was a security header's 6.x key, and the header it set would not be \
                      sent as set",
            });
        }
    }
    Ok(())
}

// The vocabularies' other 6.x keys: losing one leaves a policy more closed
// than the one set, so the report says it at `warn`.
nest_rs_core::inventory::submit! {
    nest_rs_config::__private::RetiredKey::new("http", "CORS_EXPOSED", "CORS_EXPOSED_HEADERS")
}
nest_rs_core::inventory::submit! {
    nest_rs_config::__private::RetiredKey::new("http", "CORS_MAX_AGE", "CORS_MAX_AGE_SECS")
}
nest_rs_core::inventory::submit! {
    nest_rs_config::__private::RetiredKey::new(
        "http",
        "SECURITY_HEADERS",
        "SECURITY_HEADERS_ENABLED",
    )
}

/// The global prefix, trimmed; blank is none.
fn global_prefix(
    env: &ConfigService,
    field: Field,
    base: Option<String>,
) -> Result<Option<String>> {
    Ok(match env.get(field.key())? {
        Some(raw) => Some(raw.trim().to_owned()).filter(|trimmed| !trimmed.is_empty()),
        None => base,
    })
}

/// The header the [`ApiVersioning::Header`] strategy reads, refused from either
/// side when it is no HTTP header name.
fn version_header(env: &ConfigService, field: Field, base: String) -> Result<String> {
    let Some(raw) = env.setting(field.key())? else {
        header_name(env.var_name(field.key()), field, &base, "is not set, and ")?;
        return Ok(base);
    };
    match HeaderName::from_bytes(raw.value.as_bytes()) {
        Ok(_) => Ok(raw.value),
        Err(_) => Err(raw.refuse("is not a valid HTTP header name")),
    }
}

/// `name`, set in code through `field`, refused under `var` when it is no HTTP
/// header name, and never quoted; `lead` opens the sentence with what is known
/// of the variable.
fn header_name(var: String, field: Field, name: &str, lead: &str) -> Result<HeaderName> {
    match HeaderName::from_bytes(name.as_bytes()) {
        Ok(header) => Ok(header),
        Err(_) => Err(nest_rs_config::ConfigError::parse(
            var,
            format!(
                "{lead}`{}` set in code is not a valid HTTP header name",
                field.path()
            ),
        )),
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use nest_rs_config::Config;

    use super::*;

    /// The transport's own long-lived streams take the defaults it exports
    /// for every edge riding on it.
    #[test]
    fn the_sse_defaults_are_the_exported_ones() {
        let d = HttpConfig::default();
        assert_eq!(d.sse_max_connection, Some(DEFAULT_MAX_CONNECTION));
        assert_eq!(d.sse_keep_alive, Some(DEFAULT_SSE_KEEP_ALIVE));
        assert_eq!(d.request_timeout, Some(DEFAULT_UNIT_TIMEOUT));
    }

    /// A TLS file the transport cannot read fails the boot with its I/O reason
    /// as the source.
    #[test]
    fn an_unreadable_tls_file_fails_the_boot_with_its_reason() {
        let env = ConfigService::with_vars(
            "http",
            [
                ("TLS_CERT_FILE", "/nonexistent/nest-rs-http/cert.pem"),
                ("TLS_KEY_FILE", "/nonexistent/nest-rs-http/key.pem"),
            ],
        );
        let error = HttpConfig::from_env(&env, HttpConfig::default())
            .expect_err("an unreadable file fails the boot");
        assert!(
            matches!(error, nest_rs_config::ConfigError::File { .. }),
            "{error}"
        );
        assert!(
            std::error::Error::source(&error).is_some(),
            "the I/O reason travels as the source: {error}",
        );
    }

    /// A value read from a `_FILE` spelling is refused under that spelling and
    /// never quoted — at every key `HttpConfig` judges itself.
    #[test]
    fn a_refused_value_from_a_file_names_its_file_variable_and_is_never_shown() {
        const SECRET: &str = "hunter2_secret";
        for (key, content) in [
            ("VERSIONING", SECRET.to_owned()),
            ("TRUSTED_PROXIES", format!("10.0.0.1,{SECRET}")),
            ("VERSION_HEADER", format!("{SECRET} bad")),
            ("TLS_RELOAD_SECS", SECRET.to_owned()),
            ("CORS_MAX_AGE_SECS", SECRET.to_owned()),
            ("SECURITY_HEADERS_HSTS", format!("{SECRET}\u{7}")),
            ("SHUTDOWN_TIMEOUT_SECS", SECRET.to_owned()),
            ("MAX_CONCURRENT_CONNECTIONS", SECRET.to_owned()),
            ("HEADER_READ_TIMEOUT_SECS", SECRET.to_owned()),
            ("IDLE_TIMEOUT_SECS", SECRET.to_owned()),
            ("SEND_TIMEOUT_SECS", SECRET.to_owned()),
            ("BODY_READ_TIMEOUT_SECS", SECRET.to_owned()),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.create_file("value", &content)?;
                let file_key = format!("{key}_FILE");
                let mut vars = vec![(file_key.as_str(), "value")];
                if key == "CORS_MAX_AGE_SECS" {
                    vars.push(("CORS_ORIGINS", "https://a.example"));
                }
                let env = ConfigService::with_vars("http", vars);
                let err = HttpConfig::from_env(&env, HttpConfig::default())
                    .expect_err("the value is refused")
                    .to_string();
                assert!(
                    err.contains(&nest_rs_config::var_name("http", &file_key)),
                    "{key}: names the file spelling: {err}"
                );
                assert!(!err.contains(SECRET), "{key}: never shows the value: {err}");
                Ok(())
            });
        }
    }

    /// An inline value is refused as a file's is, by its variable and never
    /// by the value — at every key `HttpConfig` judges itself.
    #[test]
    fn a_refused_inline_value_names_its_variable_and_is_never_shown() {
        const SECRET: &str = "hunter2_secret";
        for (key, value) in [
            ("VERSIONING", SECRET.to_owned()),
            ("TRUSTED_PROXIES", format!("10.0.0.1,{SECRET}")),
            ("VERSION_HEADER", format!("{SECRET} bad")),
            ("TLS_RELOAD_SECS", SECRET.to_owned()),
            ("CORS_MAX_AGE_SECS", SECRET.to_owned()),
            ("SECURITY_HEADERS_HSTS", format!("{SECRET}\u{7}")),
            ("PORT", SECRET.to_owned()),
            ("SHUTDOWN_TIMEOUT_SECS", SECRET.to_owned()),
            ("MAX_CONCURRENT_CONNECTIONS", SECRET.to_owned()),
            ("HEADER_READ_TIMEOUT_SECS", SECRET.to_owned()),
            ("IDLE_TIMEOUT_SECS", SECRET.to_owned()),
            ("SEND_TIMEOUT_SECS", SECRET.to_owned()),
        ] {
            let mut vars = vec![(key, value.as_str())];
            if key == "CORS_MAX_AGE_SECS" {
                vars.push(("CORS_ORIGINS", "https://a.example"));
            }
            let env = ConfigService::with_vars("http", vars);
            let err = HttpConfig::from_env(&env, HttpConfig::default())
                .expect_err("the value is refused")
                .to_string();
            assert!(
                err.contains(&nest_rs_config::var_name("http", key)),
                "{key}: names the variable: {err}"
            );
            assert!(!err.contains(SECRET), "{key}: never shows the value: {err}");
        }
    }

    /// A vocabulary's keys are its fields' names under the field holding it —
    /// `CORS_EXPOSED_HEADERS` for `cors.exposed_headers`, the security
    /// headers' switch `SECURITY_HEADERS_ENABLED` — and the 6.x spellings
    /// `CORS_EXPOSED` and the bare `SECURITY_HEADERS` are read by nothing.
    #[test]
    fn a_vocabularys_keys_are_its_fields_names_under_the_field_holding_it() {
        let read = |vars: &[(&str, &str)]| {
            let mut vars = vars.to_vec();
            vars.push(("CORS_ORIGINS", "https://a.example"));
            HttpConfig::from_env(
                &ConfigService::with_vars("http", vars),
                HttpConfig::default(),
            )
            .expect("the config reads")
        };
        let read_off_fields = read(&[
            ("CORS_EXPOSED_HEADERS", "x-cost"),
            ("SECURITY_HEADERS_ENABLED", "false"),
            ("SECURITY_HEADERS_FRAME_OPTIONS", "SAMEORIGIN"),
            ("SECURITY_HEADERS_HSTS", "off"),
        ]);
        let cors = read_off_fields.cors.expect("origins are set");
        assert_eq!(cors.exposed_headers, ["x-cost"]);
        let headers = read_off_fields.security_headers;
        assert!(!headers.enabled);
        assert_eq!(headers.frame_options.as_deref(), Some("SAMEORIGIN"));
        assert_eq!(headers.hsts, None);

        let retired = read(&[("CORS_EXPOSED", "x-cost"), ("SECURITY_HEADERS", "false")]);
        assert!(
            retired
                .cors
                .expect("origins are set")
                .exposed_headers
                .is_empty()
        );
        assert_eq!(retired.security_headers, HttpSecurityHeaders::default());
    }

    /// A header a deployment set under its 6.x flat key would be sent as its
    /// default, or not at all: the boot refuses it, naming the key it became
    /// and never the value.
    #[test]
    fn a_security_header_set_under_its_6x_key_is_refused_naming_its_7_0_key() {
        const SECRET: &str = "default-src 'self' hunter2_secret";
        for retired in RETIRED_HEADER_KEYS {
            let refused = HttpConfig::from_env(
                &ConfigService::with_vars("http", [(retired, SECRET)]),
                HttpConfig::default(),
            )
            .expect_err("a retired key is refused");
            assert!(
                matches!(&refused, nest_rs_config::ConfigError::Retired { .. }),
                "a key is refused, never its value: {refused:?}"
            );
            let refused = refused.to_string();
            assert!(
                refused.starts_with(&format!(
                    "{} is read by nothing since 7.0 — set {}:",
                    nest_rs_config::var_name("http", retired),
                    nest_rs_config::var_name("http", &format!("SECURITY_HEADERS_{retired}")),
                )),
                "{refused}"
            );
            assert!(!refused.contains(SECRET), "{refused}");
        }
    }

    #[test]
    fn defaults_bind_all_interfaces_on_3000_with_no_tls_no_cors() {
        let d = HttpConfig::default();
        assert_eq!(d.host, "0.0.0.0");
        assert_eq!(d.port, 3000);
        assert!(d.tls.is_none(), "default must serve plain HTTP");
        assert!(d.cors.is_none(), "no CORS layer by default");
        assert!(
            !d.server_header,
            "Server header opt-out by default — no framework fingerprint in prod",
        );
        assert!(d.global_prefix.is_none(), "no global prefix by default");
        assert_eq!(d.max_body_bytes, RawBody::DEFAULT_LIMIT);
        assert!(
            !d.compression,
            "compression opt-in — proxy-terminated by default"
        );
        assert!(
            d.trusted_proxies.is_empty(),
            "no proxy is believed until the deployment names one",
        );
    }

    #[test]
    fn trusted_proxies_are_parsed_from_the_env_list() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                nest_rs_config::var_name("http", "TRUSTED_PROXIES"),
                "10.0.0.1, 2001:db8::1,10.0.0.2",
            );
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env, Default::default()).expect("env parses");
            assert_eq!(
                cfg.trusted_proxies,
                vec![
                    "10.0.0.1".parse::<IpAddr>().unwrap(),
                    "2001:db8::1".parse().unwrap(),
                    "10.0.0.2".parse().unwrap(),
                ],
            );
            Ok(())
        });
    }

    #[test]
    fn an_unparseable_trusted_proxy_fails_instead_of_being_skipped() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                nest_rs_config::var_name("http", "TRUSTED_PROXIES"),
                "10.0.0.1,10.0.0.oops",
            );
            let env = ConfigService::for_namespace("http");
            let err = HttpConfig::from_env(&env, Default::default())
                .expect_err("a bad IP must abort the boot");
            let msg = err.to_string();
            assert!(msg.contains("TRUSTED_PROXIES"), "{msg}");
            assert!(
                msg.contains("entry 2 of the list is not an IP address"),
                "names the entry by its place, not its value: {msg}"
            );
            assert!(!msg.contains("10.0.0.oops"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn a_pinned_trusted_proxy_list_survives_an_unrelated_env_override() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::var_name("http", "PORT"), "8080");
            let pinned = HttpConfig {
                trusted_proxies: vec!["10.0.0.9".parse().unwrap()],
                ..Default::default()
            };
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env, pinned).expect("env parses");
            assert_eq!(cfg.port, 8080);
            assert_eq!(
                cfg.trusted_proxies,
                vec!["10.0.0.9".parse::<IpAddr>().unwrap()]
            );
            Ok(())
        });
    }

    #[test]
    fn from_env_reads_compression_flag() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::var_name("http", "COMPRESSION"), "true");
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env, Default::default()).expect("env parses");
            assert!(cfg.compression);
            Ok(())
        });
    }

    #[test]
    fn a_pinned_port_does_not_freeze_the_rest_of_the_namespace() {
        let pinned = HttpConfig {
            port: 3000,
            ..Default::default()
        };
        let cfg = HttpConfig::from_env(
            &ConfigService::with_vars(
                "http",
                [
                    ("PORT", "3555"),
                    ("GLOBAL_PREFIX", "/api"),
                    ("MAX_BODY_BYTES", "4096"),
                    ("COMPRESSION", "true"),
                ],
            ),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.port, 3555, "the env outranks the pinned port");
        assert_eq!(cfg.global_prefix.as_deref(), Some("/api"));
        assert_eq!(cfg.max_body_bytes, 4096);
        assert!(cfg.compression);
        assert_eq!(cfg.host, "0.0.0.0", "and an untouched field keeps the base");
    }

    #[test]
    fn a_pinned_field_survives_a_silent_environment() {
        let pinned = HttpConfig {
            global_prefix: Some("/pinned".into()),
            ..Default::default()
        };
        let cfg = HttpConfig::from_env(&ConfigService::with_vars("http", []), pinned)
            .expect("the overlay resolves");
        assert_eq!(
            cfg.global_prefix.as_deref(),
            Some("/pinned"),
            "nothing in the env ⇒ the pin is the answer",
        );
    }

    /// A set `FAIL_SECURE_STRICT` changes nothing, and the boot names it as a
    /// key no config reads.
    #[test]
    fn the_fail_secure_strict_variable_is_no_longer_read() {
        let var = nest_rs_config::var_name("http", "FAIL_SECURE_STRICT");
        figment::Jail::expect_with(|jail| {
            jail.set_env(&var, "false");
            let logs = nest_rs_testing::LogCapture::install();
            let read = HttpConfig::resolve(None).expect("an unread variable is no refusal");
            assert_eq!(
                format!("{read:?}"),
                format!("{:?}", HttpConfig::defaults()),
                "a set value changes nothing",
            );
            let line = logs.expect_one(
                nest_rs_config::TARGET,
                nest_rs_config::unclaimed::UNREAD_CONFIG_VARIABLE,
            );
            assert_eq!(line.field("variable").as_deref(), Some(var.as_str()));
            Ok(())
        });
    }

    /// The 6.x spellings of the vocabularies' keys are read by nothing, and
    /// the boot says each once as retired, naming the key it became — the
    /// bare `SECURITY_HEADERS`, which leads every security header's key, the
    /// switch's.
    #[test]
    fn a_retired_vocabulary_key_is_named_at_boot() {
        let var = |key| nest_rs_config::var_name("http", key);
        figment::Jail::expect_with(|jail| {
            jail.set_env(var("CORS_ORIGINS"), "https://a.example");
            for retired in ["CORS_EXPOSED", "CORS_MAX_AGE", "SECURITY_HEADERS"] {
                jail.set_env(var(retired), "off");
            }
            let logs = nest_rs_testing::LogCapture::install();
            HttpConfig::resolve(None).expect("an unread variable is no refusal");
            let lines = logs.find(nest_rs_config::TARGET, nest_rs_core::RETIRED_VARIABLE);
            let named = |retired| {
                lines
                    .iter()
                    .find(|line| line.field("variable") == Some(var(retired)))
                    .unwrap_or_else(|| panic!("{retired} is said retired: {lines:#?}"))
                    .field("read_as")
            };
            assert_eq!(named("CORS_MAX_AGE"), Some(var("CORS_MAX_AGE_SECS")));
            assert_eq!(named("CORS_EXPOSED"), Some(var("CORS_EXPOSED_HEADERS")));
            assert_eq!(
                named("SECURITY_HEADERS"),
                Some(var("SECURITY_HEADERS_ENABLED"))
            );
            assert!(
                logs.find(
                    nest_rs_config::TARGET,
                    nest_rs_config::unclaimed::UNREAD_CONFIG_VARIABLE,
                )
                .is_empty(),
                "a retired key is said once, as retired"
            );
            Ok(())
        });
    }

    /// A security header's 6.x key read from the environment fails the boot,
    /// where an unread variable is only said at `warn`.
    #[test]
    fn a_security_headers_6x_key_in_the_environment_fails_the_boot() {
        let var = |key| nest_rs_config::var_name("http", key);
        figment::Jail::expect_with(|jail| {
            jail.set_env(var("CONTENT_SECURITY_POLICY"), "default-src 'self'");
            let refused = HttpConfig::resolve(None)
                .expect_err("a retired security header key fails the boot")
                .to_string();
            assert!(
                refused.contains(&var("SECURITY_HEADERS_CONTENT_SECURITY_POLICY")),
                "{refused}"
            );
            Ok(())
        });
    }

    #[test]
    fn default_constants_do_not_drift() {
        assert_eq!(DEFAULT_HOST, "0.0.0.0");
        assert_eq!(DEFAULT_PORT, 3000);
    }

    #[test]
    fn from_env_reads_global_prefix() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::var_name("http", "GLOBAL_PREFIX"), "/api");
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env, Default::default()).expect("env parses");
            assert_eq!(cfg.global_prefix.as_deref(), Some("/api"));
            Ok(())
        });
    }

    #[test]
    fn from_env_treats_blank_global_prefix_as_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::var_name("http", "GLOBAL_PREFIX"), "   ");
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env, Default::default()).expect("env parses");
            assert!(cfg.global_prefix.is_none());
            Ok(())
        });
    }

    #[test]
    fn from_env_reads_max_body_bytes() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::var_name("http", "MAX_BODY_BYTES"), "1024");
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env, Default::default()).expect("env parses");
            assert_eq!(cfg.max_body_bytes, 1024);
            Ok(())
        });
    }

    #[test]
    fn a_zero_body_cap_fails_the_boot_rather_than_rejecting_every_request() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::var_name("http", "MAX_BODY_BYTES"), "0");
            let err = HttpConfig::resolve(None).expect_err("a zero-byte cap must abort the boot");
            let msg = err.to_string();
            assert!(msg.contains("max_body_bytes"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn a_zero_request_timeout_turns_the_budget_off_rather_than_zeroing_it() {
        let cfg = HttpConfig::from_env(
            &ConfigService::with_vars("http", [("REQUEST_TIMEOUT_SECS", "0")]),
            Default::default(),
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.request_timeout, None, "`0` is the off sentinel");
    }

    #[test]
    fn the_request_timeout_reads_whole_seconds_over_a_pinned_base() {
        let pinned = HttpConfig {
            request_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        assert_eq!(
            HttpConfig::from_env(&ConfigService::with_vars("http", []), pinned.clone())
                .expect("the overlay resolves")
                .request_timeout,
            Some(Duration::from_secs(5)),
            "nothing in the env ⇒ the pin is the answer",
        );
        assert_eq!(
            HttpConfig::from_env(
                &ConfigService::with_vars("http", [("REQUEST_TIMEOUT_SECS", "15")]),
                pinned,
            )
            .expect("the overlay resolves")
            .request_timeout,
            Some(Duration::from_secs(15)),
        );
    }

    #[test]
    fn the_shutdown_window_defaults_to_20s_and_reads_whole_seconds_over_a_pinned_base() {
        assert_eq!(
            HttpConfig::default().shutdown_timeout,
            Duration::from_secs(20)
        );
        let pinned = HttpConfig {
            shutdown_timeout: Duration::from_secs(10),
            ..Default::default()
        };
        assert_eq!(
            HttpConfig::from_env(&ConfigService::with_vars("http", []), pinned.clone())
                .expect("the overlay resolves")
                .shutdown_timeout,
            Duration::from_secs(10),
            "nothing in the env ⇒ the pin is the answer",
        );
        assert_eq!(
            HttpConfig::from_env(
                &ConfigService::with_vars("http", [("SHUTDOWN_TIMEOUT_SECS", "40")]),
                pinned,
            )
            .expect("the overlay resolves")
            .shutdown_timeout,
            Duration::from_secs(40),
        );
    }

    /// `0` is refused here, not read as the shared off.
    #[test]
    fn a_shutdown_window_outside_its_range_is_refused_naming_the_variable() {
        let var = nest_rs_config::var_name("http", "SHUTDOWN_TIMEOUT_SECS");
        for (value, reason) in [
            ("0", "must be at least 1"),
            ("3601", "must be at most 3600"),
        ] {
            let refused = HttpConfig::from_env(
                &ConfigService::with_vars("http", [("SHUTDOWN_TIMEOUT_SECS", value)]),
                HttpConfig::default(),
            )
            .expect_err("refused")
            .to_string();
            assert!(refused.contains(&var), "{value}: {refused}");
            assert!(refused.contains(reason), "{value}: {refused}");
        }
        for (value, secs) in [("1", 1), ("3600", 3600)] {
            let cfg = HttpConfig::from_env(
                &ConfigService::with_vars("http", [("SHUTDOWN_TIMEOUT_SECS", value)]),
                HttpConfig::default(),
            )
            .expect("an edge of the range is inside it");
            assert_eq!(cfg.shutdown_timeout, Duration::from_secs(secs));
        }
    }

    /// Refused only while the variable is silent: a set variable overrides the pin.
    #[test]
    fn a_pinned_shutdown_window_outside_its_range_is_refused_naming_the_field() {
        let var = nest_rs_config::var_name("http", "SHUTDOWN_TIMEOUT_SECS");
        for pinned in [
            Duration::ZERO,
            Duration::from_millis(999),
            Duration::from_secs(3601),
        ] {
            let refused = HttpConfig::from_env(
                &ConfigService::with_vars("http", []),
                HttpConfig {
                    shutdown_timeout: pinned,
                    ..Default::default()
                },
            )
            .expect_err("refused")
            .to_string();
            assert!(refused.contains(&var), "{pinned:?}: {refused}");
            assert!(
                refused.contains("HttpConfig::shutdown_timeout"),
                "{pinned:?}: {refused}"
            );
        }
        let cfg = HttpConfig::from_env(
            &ConfigService::with_vars("http", [("SHUTDOWN_TIMEOUT_SECS", "5")]),
            HttpConfig {
                shutdown_timeout: Duration::ZERO,
                ..Default::default()
            },
        )
        .expect("the variable overrides the pin");
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(5));
    }

    /// Refused whatever the strategy and never quoted, while the variable is
    /// silent and by the selector a config built in code describes; a set
    /// variable overrides the pin.
    #[test]
    fn a_pinned_version_header_that_is_no_header_name_is_refused_naming_the_field() {
        let var = nest_rs_config::var_name("http", "VERSION_HEADER");
        for versioning in [ApiVersioning::Header, ApiVersioning::Uri] {
            let pinned = HttpConfig {
                versioning,
                version_header: "X S3cr3t".into(),
                ..Default::default()
            };
            for refused in [
                HttpConfig::from_env(&ConfigService::with_vars("http", []), pinned.clone())
                    .expect_err("refused")
                    .to_string(),
                pinned.version_selector().expect_err("refused").to_string(),
            ] {
                assert!(refused.contains(&var), "{versioning:?}: {refused}");
                assert!(
                    refused.contains("HttpConfig::version_header"),
                    "{versioning:?}: {refused}"
                );
                assert!(!refused.contains("S3cr3t"), "{versioning:?}: {refused}");
            }
        }
        let read = HttpConfig::from_env(
            &ConfigService::with_vars("http", [("VERSION_HEADER", "X-Version")]),
            HttpConfig {
                version_header: "X Version".into(),
                ..Default::default()
            },
        )
        .expect("the variable overrides the pin");
        assert_eq!(read.version_header, "X-Version");
    }

    #[test]
    fn the_connection_cap_defaults_to_ten_thousand_and_reads_over_a_pinned_base() {
        assert_eq!(HttpConfig::default().max_concurrent_connections, 10_000);
        let pinned = HttpConfig {
            max_concurrent_connections: 64,
            ..Default::default()
        };
        assert_eq!(
            HttpConfig::from_env(&ConfigService::with_vars("http", []), pinned.clone())
                .expect("the overlay resolves")
                .max_concurrent_connections,
            64,
            "nothing in the env ⇒ the pin is the answer",
        );
        assert_eq!(
            HttpConfig::from_env(
                &ConfigService::with_vars("http", [("MAX_CONCURRENT_CONNECTIONS", "512")]),
                pinned,
            )
            .expect("the overlay resolves")
            .max_concurrent_connections,
            512,
        );
    }

    /// `0` is no off switch here: a cap of none would bind and accept nothing.
    #[test]
    fn a_connection_cap_outside_its_range_is_refused_naming_the_variable() {
        let var = nest_rs_config::var_name("http", "MAX_CONCURRENT_CONNECTIONS");
        for (value, reason) in [
            ("0", "must be at least 1"),
            ("1048577", "must be at most 1048576"),
        ] {
            let refused = HttpConfig::from_env(
                &ConfigService::with_vars("http", [("MAX_CONCURRENT_CONNECTIONS", value)]),
                HttpConfig::default(),
            )
            .expect_err("refused")
            .to_string();
            assert!(refused.contains(&var), "{value}: {refused}");
            assert!(refused.contains(reason), "{value}: {refused}");
            assert!(
                !refused.contains(&format!(" {value}")),
                "{value}: the value read is never quoted: {refused}"
            );
        }
        for (value, count) in [("1", 1), ("1048576", 1_048_576)] {
            let cfg = HttpConfig::from_env(
                &ConfigService::with_vars("http", [("MAX_CONCURRENT_CONNECTIONS", value)]),
                HttpConfig::default(),
            )
            .expect("an edge of the range is inside it");
            assert_eq!(cfg.max_concurrent_connections, count);
        }
    }

    /// Refused only while the variable is silent: a set variable overrides the pin.
    #[test]
    fn a_pinned_connection_cap_outside_its_range_is_refused_naming_the_field() {
        let var = nest_rs_config::var_name("http", "MAX_CONCURRENT_CONNECTIONS");
        for pinned in [0, 1_048_577] {
            let refused = HttpConfig::from_env(
                &ConfigService::with_vars("http", []),
                HttpConfig {
                    max_concurrent_connections: pinned,
                    ..Default::default()
                },
            )
            .expect_err("refused")
            .to_string();
            assert!(refused.contains(&var), "{pinned}: {refused}");
            assert!(
                refused.contains("HttpConfig::max_concurrent_connections"),
                "{pinned}: {refused}"
            );
        }
        let cfg = HttpConfig::from_env(
            &ConfigService::with_vars("http", [("MAX_CONCURRENT_CONNECTIONS", "5")]),
            HttpConfig {
                max_concurrent_connections: 0,
                ..Default::default()
            },
        )
        .expect("the variable overrides the pin");
        assert_eq!(cfg.max_concurrent_connections, 5);
    }

    /// The "default is ordered" half of the rule: a request admitted just
    /// before a stop is answered inside the window.
    #[test]
    fn the_default_request_timeout_fits_the_default_shutdown_window() {
        let defaults = HttpConfig::default();
        let budget = defaults
            .request_timeout
            .expect("a request is bounded by default");
        assert!(
            budget <= defaults.shutdown_timeout,
            "{budget:?} outlasts the {:?} window",
            defaults.shutdown_timeout,
        );
    }

    #[test]
    fn the_connection_deadlines_default_to_30_10_75_and_60_seconds() {
        let defaults = HttpConfig::default();
        assert_eq!(defaults.header_read_timeout, Duration::from_secs(30));
        assert_eq!(defaults.body_read_timeout, Duration::from_secs(10));
        assert_eq!(defaults.idle_timeout, Duration::from_secs(75));
        assert_eq!(defaults.send_timeout, Duration::from_secs(60));
    }

    /// Where a deadline sits in [`HttpConfig`].
    type Slot = fn(&mut HttpConfig) -> &mut Duration;

    /// Each deadline's key, the field pinning it, its ceiling in seconds, and
    /// its slot.
    const DEADLINES: [(&str, &str, u64, Slot); 4] = [
        (
            "HEADER_READ_TIMEOUT_SECS",
            "HttpConfig::header_read_timeout",
            300,
            |config| &mut config.header_read_timeout,
        ),
        (
            "BODY_READ_TIMEOUT_SECS",
            "HttpConfig::body_read_timeout",
            3600,
            |config| &mut config.body_read_timeout,
        ),
        (
            "IDLE_TIMEOUT_SECS",
            "HttpConfig::idle_timeout",
            3600,
            |config| &mut config.idle_timeout,
        ),
        (
            "SEND_TIMEOUT_SECS",
            "HttpConfig::send_timeout",
            3600,
            |config| &mut config.send_timeout,
        ),
    ];

    /// `HttpConfig::default()` with the deadline in `slot` pinned to `value`.
    fn pinning(slot: Slot, value: Duration) -> HttpConfig {
        let mut config = HttpConfig::default();
        *slot(&mut config) = value;
        config
    }

    /// `0` is no off switch for any of them: a phase without a deadline is the
    /// hole they close.
    #[test]
    fn a_connection_deadline_outside_its_range_is_refused_naming_its_variable() {
        for (key, _, ceiling, slot) in DEADLINES {
            let var = nest_rs_config::var_name("http", key);
            for (value, reason) in [
                ("0".to_owned(), "must be at least 1".to_owned()),
                (
                    (ceiling + 1).to_string(),
                    format!("must be at most {ceiling}"),
                ),
            ] {
                let refused = HttpConfig::from_env(
                    &ConfigService::with_vars("http", [(key, value.as_str())]),
                    HttpConfig::default(),
                )
                .expect_err("refused")
                .to_string();
                assert!(refused.contains(&var), "{key}={value}: {refused}");
                assert!(refused.contains(&reason), "{key}={value}: {refused}");
            }
            for secs in [1, ceiling] {
                let mut read = HttpConfig::from_env(
                    &ConfigService::with_vars("http", [(key, secs.to_string().as_str())]),
                    pinning(slot, Duration::from_secs(7)),
                )
                .expect("an edge of the range is inside it");
                assert_eq!(*slot(&mut read), Duration::from_secs(secs), "{key}");
            }
        }
    }

    /// Refused only while the variable is silent, and by the transport built
    /// from a config that never met the environment.
    #[test]
    fn a_pinned_connection_deadline_outside_its_range_is_refused_naming_its_field() {
        for (key, field, ceiling, slot) in DEADLINES {
            let var = nest_rs_config::var_name("http", key);
            for pinned in [
                Duration::ZERO,
                Duration::from_millis(999),
                Duration::from_secs(ceiling + 1),
            ] {
                for refused in [
                    HttpConfig::from_env(
                        &ConfigService::with_vars("http", []),
                        pinning(slot, pinned),
                    )
                    .expect_err("refused")
                    .to_string(),
                    pinning(slot, pinned)
                        .deadlines()
                        .expect_err("refused")
                        .to_string(),
                ] {
                    assert!(refused.contains(&var), "{key} {pinned:?}: {refused}");
                    assert!(refused.contains(field), "{key} {pinned:?}: {refused}");
                }
            }
            let mut read = HttpConfig::from_env(
                &ConfigService::with_vars("http", [(key, "5")]),
                pinning(slot, Duration::ZERO),
            )
            .expect("the variable overrides the pin");
            assert_eq!(*slot(&mut read), Duration::from_secs(5), "{key}");
        }
    }

    #[test]
    fn from_env_rejects_unparseable_max_body_bytes() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::var_name("http", "MAX_BODY_BYTES"), "huge");
            let env = ConfigService::for_namespace("http");
            assert!(
                HttpConfig::from_env(&env, Default::default()).is_err(),
                "non-numeric must surface as ConfigError — no silent default",
            );
            Ok(())
        });
    }
}

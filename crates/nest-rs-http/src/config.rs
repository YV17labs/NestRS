use std::net::IpAddr;
use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Result, config};
use poem::http::HeaderName;

use crate::cors::CorsConfig;
use crate::raw_body::RawBody;
use crate::security_headers::SecurityHeadersConfig;
use crate::tls::TlsConfig;
use crate::versioning::{ApiVersioning, DEFAULT_VERSION_HEADER, VersionSelector};

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 3000;
/// Default `#[sse]` stream ceiling: 4 hours — the same value
/// `NESTRS_WS__MAX_CONNECTION_SECS` and `NESTRS_GRAPHQL__MAX_CONNECTION_SECS`
/// default to, because it bounds the same stale-privilege window.
const DEFAULT_SSE_MAX_CONNECTION_SECS: u64 = 4 * 60 * 60;
/// Default keep-alive comment interval on a `#[sse]` stream: 15 seconds, under
/// the 30–60 s idle timeout common to proxies and browsers.
const DEFAULT_SSE_KEEP_ALIVE_SECS: u64 = 15;

/// HTTP transport options resolved at boot. Every field is settable both via
/// `NESTRS_HTTP__*` env vars (read by [`Config::from_env`]) and via the pinned
/// struct (passed to [`HttpModule::for_root`](crate::HttpModule::for_root)) —
/// and the two compose **per field**: a pinned struct is the base the
/// environment overlays, so pinning `port` leaves `NESTRS_HTTP__TLS_CERT_FILE`
/// and every other `NESTRS_HTTP__*` key live. See [`Config`] for the full
/// precedence chain.
#[config(namespace = "http")]
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// Bind address; defaults to `0.0.0.0` (all interfaces).
    pub host: String,
    /// Listen port; defaults to `3000`.
    pub port: u16,
    /// PEM cert + key for HTTPS. `None` ⇒ plain HTTP. Picked up from
    /// `NESTRS_HTTP__TLS_CERT[_FILE]` + `NESTRS_HTTP__TLS_KEY[_FILE]`.
    pub tls: Option<TlsConfig>,
    /// CORS policy. `None` ⇒ no CORS layer. Populated when
    /// `NESTRS_HTTP__CORS_ORIGINS` is set (see [`CorsConfig`]).
    pub cors: Option<CorsConfig>,
    /// How a caller selects an API version. `uri` (the default) reads it from
    /// the path a `#[controller(version = …)]` mounts at; `header` and
    /// `media_type` resolve it per request instead. Read from
    /// `NESTRS_HTTP__VERSIONING`.
    pub versioning: ApiVersioning,
    /// The header [`ApiVersioning::Header`] reads. Defaults to
    /// `X-API-Version`; read from `NESTRS_HTTP__VERSION_HEADER`. Ignored by the
    /// other two strategies.
    pub version_header: String,
    /// The version served to a caller that states none. `None` (the default)
    /// leaves such a request on the unversioned routes. Read from
    /// `NESTRS_HTTP__DEFAULT_VERSION`.
    pub default_version: Option<String>,
    /// `true` ⇒ emit `Server: nestrs/<version>` on every response.
    /// Defaults to `false` (production-safe — no framework fingerprint).
    /// Flip to `true` in `.env.development` to expose the version locally.
    pub server_header: bool,
    /// Mount every controller under a shared path prefix (e.g. `/api`). `None`
    /// ⇒ no prefix. Read from `NESTRS_HTTP__GLOBAL_PREFIX`; normalization
    /// (trim, drop empty/`"/"`, ensure leading `/`, strip trailing `/`) lives
    /// in [`HttpTransport::global_prefix`](crate::HttpTransport::global_prefix).
    pub global_prefix: Option<String>,
    /// Transport-wide cap on the request body size. Enforced at the transport
    /// edge for **every** extractor — a bare `Json`/`String`/`Vec<u8>`/
    /// `Multipart`, not only [`RawBody`](crate::RawBody) — via a fast `413` on
    /// an oversized `Content-Length` plus a streaming cap for chunked bodies. A
    /// per-route [`RawBody::extract_with_limit`](crate::RawBody::extract_with_limit)
    /// can pin a *tighter* cap under this ceiling. `None` ⇒ the default (2 MiB).
    /// Read from `NESTRS_HTTP__MAX_BODY_BYTES`.
    pub max_body_bytes: Option<usize>,
    /// Wall-clock budget for a single request. A handler exceeding it is
    /// aborted and the client gets `504 Gateway Timeout`, bounding how long a
    /// slow/stuck request ties up a connection. `None` ⇒ no timeout. Read from
    /// `NESTRS_HTTP__REQUEST_TIMEOUT_SECS`. Default `30`.
    pub request_timeout_secs: Option<u64>,
    /// `true` (the default) fails boot when global guards are registered and
    /// an endpoint the transport cannot shape (an imperative `mount(...)`)
    /// would bypass the guard pool; `false` downgrades to a `warn`. Read from
    /// `NESTRS_HTTP__FAIL_SECURE_STRICT`.
    pub fail_secure_strict: bool,
    /// Default security response headers (`nosniff`, `X-Frame-Options`, HSTS
    /// under TLS). On by default; tune via `NESTRS_HTTP__SECURITY_HEADERS`,
    /// `__FRAME_OPTIONS`, `__HSTS`, `__CONTENT_TYPE_OPTIONS`.
    pub security_headers: SecurityHeadersConfig,
    /// Negotiate response compression (gzip / deflate / brotli / zstd) from the
    /// request's `Accept-Encoding`. Off by default — leave it to the reverse
    /// proxy in most deployments; flip on with `NESTRS_HTTP__COMPRESSION=true`
    /// when the app terminates responses directly.
    pub compression: bool,
    /// Emit one access event per request on `nest_rs::operation` — method, path,
    /// status, byte-exact size, duration, client, `trace_id` and `span_id`, plus
    /// `actor_id` once an authn guard resolved a principal. **On by default**,
    /// and read from `NESTRS_HTTP__ACCESS_LOG`.
    ///
    /// It belongs to the transport rather than to `nest-rs-opentelemetry`
    /// because everything it reports is what *this* transport knows about a
    /// request it served: no collector, no exporter and no propagator is
    /// involved, so none may be required. Turning it off does **not** turn off
    /// the correlation id — that is minted and echoed on every request either
    /// way, because it is what everything else is filed under.
    pub access_log: bool,
    /// Reverse proxies whose `X-Forwarded-For` / `X-Real-IP` this deployment
    /// believes. Empty by default — with no entry the framework reads neither
    /// header and every caller is identified by its transport peer, which is
    /// the only value a client cannot forge.
    ///
    /// Name the balancer here (`NESTRS_HTTP__TRUSTED_PROXIES=10.0.0.1,10.0.0.2`)
    /// and both [`ClientIp`](crate::ClientIp) and the throttler's rate-limit
    /// bucket start resolving the real client behind it — one list, so the two
    /// can never disagree about who a request came from. An unparseable entry
    /// **aborts the boot** naming the variable. See
    /// [`ClientOrigin`](crate::ClientOrigin) for the resolution rule.
    pub trusted_proxies: Vec<IpAddr>,
    /// Maximum lifetime of a single `#[sse]` stream. When it elapses the server
    /// stops emitting and ends the stream, so the client's `EventSource`
    /// reconnects — re-running the guard chain, and with it authn/authz and the
    /// token's `exp`. It bounds **emission**: no event is produced past the
    /// ceiling at any rate a peer reads at. A peer that stops reading parks the
    /// write and keeps its *socket* past the ceiling — a reported gap, not a
    /// closed one; see [`SseSettings`](crate::SseSettings) for why it cannot be
    /// closed from this crate, and bound idle sockets at the server or proxy.
    ///
    /// **A security control, not a resource knob**, and the third instance of
    /// the same one: a stream authenticates once, at the request, then emits
    /// with those privileges for as long as it lives. Same reading, same
    /// 4-hour default and same `0` ⇒ unlimited spelling as
    /// `NESTRS_WS__MAX_CONNECTION_SECS` and
    /// `NESTRS_GRAPHQL__MAX_CONNECTION_SECS`. Read from
    /// `NESTRS_HTTP__SSE_MAX_CONNECTION_SECS`; the `http` namespace because SSE
    /// is a response shape of this transport rather than a module of its own.
    pub sse_max_connection: Option<Duration>,
    /// How often a `#[sse]` stream emits a keep-alive comment, so an idle
    /// stream is not dropped by an intermediary that sees no bytes. `None` ⇒
    /// none sent. Read from `NESTRS_HTTP__SSE_KEEP_ALIVE_SECS` (whole seconds;
    /// `0` ⇒ none); defaults to 15 seconds.
    pub sse_keep_alive: Option<Duration>,
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
            max_body_bytes: Some(RawBody::DEFAULT_LIMIT),
            request_timeout_secs: Some(30),
            fail_secure_strict: true,
            security_headers: SecurityHeadersConfig::default(),
            compression: false,
            // On by default. A deployment that serves requests and records none
            // of them is the surprising configuration, not the reverse.
            access_log: true,
            trusted_proxies: Vec::new(),
            sse_max_connection: Some(Duration::from_secs(DEFAULT_SSE_MAX_CONNECTION_SECS)),
            sse_keep_alive: Some(Duration::from_secs(DEFAULT_SSE_KEEP_ALIVE_SECS)),
        }
    }
}

impl HttpConfig {
    /// The version selector this configuration describes, or `None` when the
    /// URI strategy is in force — routing already resolves that one, so there
    /// is nothing for the transport to wrap.
    ///
    /// The header name is validated at boot ([`Config::from_env`]), so this
    /// cannot fail here.
    pub(crate) fn version_selector(&self) -> Option<VersionSelector> {
        let header = HeaderName::from_bytes(self.version_header.as_bytes()).ok()?;
        let selector = VersionSelector::new(self.versioning, header, self.default_version.clone());
        selector.rewrites().then_some(selector)
    }

    /// Pin the global prefix in code. Empty / `"/"` collapse to `None` via the
    /// transport's normalization, so callers can pass user-provided strings
    /// without sanitizing first.
    pub fn with_global_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.global_prefix = Some(prefix.into());
        self
    }

    /// Pin the [`RawBody`](crate::RawBody) byte cap in code. Applies to every
    /// extractor invocation that does not pick its own limit via
    /// [`RawBody::extract_with_limit`](crate::RawBody::extract_with_limit).
    pub fn with_max_body_bytes(mut self, n: usize) -> Self {
        self.max_body_bytes = Some(n);
        self
    }
}

impl Config for HttpConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let global_prefix = env
            .get("GLOBAL_PREFIX")
            .map(|raw| {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            })
            .unwrap_or(base.global_prefix);
        Ok(Self {
            host: env.get("HOST").unwrap_or(base.host),
            port: env.parse("PORT")?.unwrap_or(base.port),
            // The reader owns the name: a literal here would cite a variable
            // the operator does not have once the app declares its own prefix.
            tls: TlsConfig::from_env(env, base.tls).map_err(|e| {
                nest_rs_config::ConfigError::parse(env.var_name("TLS_*"), e.to_string())
            })?,
            cors: CorsConfig::from_env(env, base.cors).map_err(|e| {
                nest_rs_config::ConfigError::parse(env.var_name("CORS_*"), e.to_string())
            })?,
            versioning: match env.get("VERSIONING") {
                Some(raw) => raw.parse().map_err(|msg: String| {
                    nest_rs_config::ConfigError::parse(env.var_name("VERSIONING"), msg)
                })?,
                None => base.versioning,
            },
            version_header: version_header(env, base.version_header)?,
            default_version: env.get("DEFAULT_VERSION").or(base.default_version),
            server_header: env.flag("SERVER_HEADER", base.server_header)?,
            global_prefix,
            max_body_bytes: env.parse("MAX_BODY_BYTES")?.or(base.max_body_bytes),
            request_timeout_secs: env
                .parse("REQUEST_TIMEOUT_SECS")?
                .or(base.request_timeout_secs),
            fail_secure_strict: env.flag("FAIL_SECURE_STRICT", base.fail_secure_strict)?,
            security_headers: SecurityHeadersConfig::from_env(env, base.security_headers)?,
            compression: env.flag("COMPRESSION", base.compression)?,
            access_log: env.flag("ACCESS_LOG", base.access_log)?,
            trusted_proxies: parse_trusted_proxies(env, base.trusted_proxies)?,
            sse_max_connection: env.seconds("SSE_MAX_CONNECTION_SECS", base.sse_max_connection)?,
            sse_keep_alive: env.seconds("SSE_KEEP_ALIVE_SECS", base.sse_keep_alive)?,
        })
    }
}

/// The header the [`ApiVersioning::Header`] strategy reads, validated here so
/// an unusable name aborts the boot naming the variable rather than silently
/// disabling version selection at the first request.
fn version_header(env: &ConfigService, base: String) -> Result<String> {
    let raw = env.get("VERSION_HEADER").unwrap_or(base);
    match HeaderName::from_bytes(raw.as_bytes()) {
        Ok(_) => Ok(raw),
        Err(_) => Err(nest_rs_config::ConfigError::parse(
            env.var_name("VERSION_HEADER"),
            format!("{raw:?} is not a valid HTTP header name"),
        )),
    }
}

/// Parse `NESTRS_HTTP__TRUSTED_PROXIES` into addresses. A typo here silently
/// disables the trust it was meant to grant — the proxy would never match the
/// peer and every caller behind it would collapse onto the balancer's address —
/// so a bad entry fails the boot naming the variable and the offending value,
/// never a silent skip.
fn parse_trusted_proxies(env: &ConfigService, base: Vec<IpAddr>) -> Result<Vec<IpAddr>> {
    const KEY: &str = "TRUSTED_PROXIES";
    // `ConfigService` has no typed-list reader, so the raw list is parsed here.
    // The pinned base short-circuits rather than round-tripping through strings.
    let Some(raw) = env.get(KEY) else {
        return Ok(base);
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<IpAddr>().map_err(|e| {
                nest_rs_config::ConfigError::parse(
                    env.var_name(KEY),
                    format!("invalid IP `{s}`: {e}"),
                )
            })
        })
        .collect()
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

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
        assert_eq!(d.max_body_bytes, Some(RawBody::DEFAULT_LIMIT));
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

    // A typo here would silently disable the trust it grants: the entry never
    // matches a peer, so every caller behind the balancer collapses onto its
    // address and the deployment looks like it has one client. Fail the boot.
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
            // The variable name is derived from the reader's namespace, not
            // retyped — a `#[config]` struct read under another namespace would
            // otherwise blame a variable nobody set.
            assert!(msg.contains("TRUSTED_PROXIES"), "{msg}");
            assert!(msg.contains("10.0.0.oops"), "{msg}");
            Ok(())
        });
    }

    // The dual-path config rule: a pinned list survives when the env sets none.
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

    // The finding: the scaffold writes `HttpConfig { port: 3000,
    // ..Default::default() }`, which used to freeze every field — a
    // deployment setting `NESTRS_HTTP__PORT` or `NESTRS_HTTP__TLS_CERT_FILE` got
    // silence. The overlay makes the pin a *base*: the env wins per field, and
    // the pin survives wherever the env is silent.
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
                    ("FAIL_SECURE_STRICT", "false"),
                ],
            ),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.port, 3555, "the env outranks the pinned port");
        assert_eq!(cfg.global_prefix.as_deref(), Some("/api"));
        assert_eq!(cfg.max_body_bytes, Some(4096));
        assert!(cfg.compression);
        assert!(!cfg.fail_secure_strict);
        assert_eq!(cfg.host, "0.0.0.0", "and an untouched field keeps the base");
    }

    #[test]
    fn a_pinned_field_survives_a_silent_environment() {
        let pinned = HttpConfig::default().with_global_prefix("/pinned");
        let cfg = HttpConfig::from_env(&ConfigService::with_vars("http", []), pinned)
            .expect("the overlay resolves");
        assert_eq!(
            cfg.global_prefix.as_deref(),
            Some("/pinned"),
            "nothing in the env ⇒ the pin is the answer",
        );
    }

    #[test]
    fn default_constants_do_not_drift() {
        // App ops read `DEFAULT_PORT` indirectly via `HttpConfig::default()` —
        // a rename or value change is a deployment surprise. Pin them.
        assert_eq!(DEFAULT_HOST, "0.0.0.0");
        assert_eq!(DEFAULT_PORT, 3000);
    }

    #[test]
    fn with_global_prefix_pins_in_code() {
        let cfg = HttpConfig::default().with_global_prefix("/api");
        assert_eq!(cfg.global_prefix.as_deref(), Some("/api"));
    }

    #[test]
    fn with_max_body_bytes_pins_in_code() {
        let cfg = HttpConfig::default().with_max_body_bytes(64);
        assert_eq!(cfg.max_body_bytes, Some(64));
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
        // `NESTRS_HTTP__GLOBAL_PREFIX=` (or whitespace) must not pin an empty
        // prefix that the transport would still try to nest under.
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
            assert_eq!(cfg.max_body_bytes, Some(1024));
            Ok(())
        });
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

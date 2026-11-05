//! [`McpConfig`] — streamable-HTTP server options for every `#[mcp]` mount.
//!
//! [`allowed_hosts`](McpConfig::allowed_hosts) is a security control: rmcp checks
//! the `Host` header against it to stop DNS rebinding, which reaches exactly the
//! local server that configures nothing — hence the loopback default. The
//! `Origin` half is checked by the endpoint itself, CORS on or off: a browser
//! request is admitted from the request's own origin while
//! `<PREFIX>_HTTP__CORS_ORIGINS` is unset, and from what it lists once set.
//! rmcp's `allowed_origins` stays empty, since it cannot say "the request's own
//! origin".
//!
//! [`operation_timeout`](McpConfig::operation_timeout) bounds an operation the
//! HTTP request timeout cannot reach: rmcp answers a session's request, and a
//! legacy stateless one, with an event stream whose head leaves at once.
//!
//! [`max_concurrent_sessions`](McpConfig::max_concurrent_sessions) and
//! [`session_idle_timeout`](McpConfig::session_idle_timeout) bound what a legacy
//! client's sessions hold: a session outlives the HTTP connection that opened it,
//! so the transport's connection cap does not count it.

use std::time::Duration;

use nest_rs_config::{Bound, CountBounds, DurationBounds, Floor, config};
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::session::local::SessionConfig;

/// rmcp's own default POST body ceiling.
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

const MAX_REQUEST_BODY_BYTES: CountBounds = CountBounds::new(
    Bound::new(1, "a cap of no bytes refuses every request a client posts"),
    Bound::new(
        1 << 30,
        "a POST is buffered whole and decoded as JSON in memory, so past a gibibyte one request \
         is what a replica holds rather than a message; a file that large goes to the object \
         store, and the tool takes its key",
    ),
);

/// Live sessions one mount holds by default.
const DEFAULT_MAX_CONCURRENT_SESSIONS: usize = 1024;

const SESSIONS: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a cap of none refuses every legacy client's `initialize`",
    ),
    Bound::new(
        1 << 20,
        "an idle session holds about 110 KiB, so a million of them hold over 100 GiB — more \
         than a replica has, where a cap that high bounds nothing; spread the clients over \
         replicas instead",
    ),
);

/// rmcp's own idle timeout today, pinned so a release of it cannot move a
/// deployment's.
const DEFAULT_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

const SESSION_IDLE_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a shorter timeout closes a session between two calls of one client, which then \
         starts over with `initialize`, and `0` is no off switch: an idle session holds its \
         worker and its place under the session cap",
    )),
    Bound::new(
        60 * 60,
        "a session idle for an hour is one its client forgot, holding its worker and its \
         place under the session cap, and a timeout that long is a unit slip more often than \
         a choice (`300000` meant as milliseconds is three days)",
    ),
);

/// How long a new session waits for its `initialize` before it is closed —
/// rmcp's default today, pinned so a release of it cannot move it.
const SESSION_INIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The advertised reconnection delay's range.
const SSE_RETRY: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a client told to come back sooner reconnects at once, and every client a restart \
         dropped does it together",
    )),
    Bound::new(
        60 * 60,
        "a client told to wait more than an hour before resuming a dropped stream has \
         abandoned the session it would resume",
    ),
);

/// MCP streamable-HTTP options resolved at boot (namespace `mcp`).
#[config(namespace = "mcp")]
#[derive(Clone, Debug)]
pub struct McpConfig {
    /// Hostnames or `host:port` authorities accepted in the inbound `Host`
    /// header (anti-DNS-rebinding). Defaults to loopback only, so a public
    /// deployment **must** name itself — `<PREFIX>_MCP__ALLOWED_HOSTS=mcp.example.com,mcp.example.com:8443`
    /// — unless it imports `OAuthResourceModule`, whose resource's host the
    /// mount admits beside these. An empty list disables the check entirely
    /// and is reported at `warn` at mount time.
    pub allowed_hosts: Vec<String>,
    /// Keep sessions alive for protocol versions older than `2026-07-28`. Per
    /// SEP-2567 the `2026-07-28` revision is always served statelessly, so this
    /// only affects legacy clients. Read from
    /// `<PREFIX>_MCP__LEGACY_SESSION_MODE`; defaults to `true`.
    pub legacy_session_mode: bool,
    /// Answer simple request/response operations with `application/json`
    /// instead of an SSE stream (the server still falls back to
    /// `text/event-stream` when a handler emits a notification first). Read
    /// from `<PREFIX>_MCP__JSON_RESPONSE`; defaults to `false`.
    pub json_response: bool,
    /// SSE keep-alive ping interval. `None` ⇒ no pings. Read from
    /// `<PREFIX>_MCP__SSE_KEEP_ALIVE_SECS`, whole seconds from 1 to 3600 or `0`
    /// for none; defaults to 15s, [`nest_rs_http::DEFAULT_SSE_KEEP_ALIVE`].
    #[config(bounds = nest_rs_http::SSE_KEEP_ALIVE_BOUNDS)]
    pub sse_keep_alive: Option<Duration>,
    /// `retry:` interval advertised on SSE priming events — a stateless
    /// answer's and every stream of a session alike. `None` ⇒ none. Read from
    /// `<PREFIX>_MCP__SSE_RETRY_SECS`, whole seconds from 1 to 3600 or `0` for
    /// none; defaults to 3s.
    #[config(bounds = SSE_RETRY)]
    pub sse_retry: Option<Duration>,
    /// Cap on a single POST body, enforced while streaming (independent of
    /// `Content-Length`); over it the client gets `413`. Read from
    /// `<PREFIX>_MCP__MAX_REQUEST_BODY_BYTES`, from 1 to a gibibyte — refused
    /// outside, from the environment and from the pinned struct alike; defaults
    /// to 4 MiB.
    #[config(bounds = MAX_REQUEST_BODY_BYTES)]
    pub max_request_body_bytes: usize,
    /// Require per-request protocol metadata (`MCP-Protocol-Version` and
    /// `_meta.io.modelcontextprotocol/protocolVersion`) on stateless request
    /// POSTs, per SEP-2243. Rejects clients negotiated below `2026-07-28`, so
    /// turn it on together with a handler that advertises only `2026-07-28`
    /// and later. Read from `<PREFIX>_MCP__STATELESS_PROTOCOL_METADATA_REQUIRED`;
    /// defaults to `false`.
    pub stateless_protocol_metadata_required: bool,
    /// Wall-clock deadline for one operation — `tools/call`, `prompts/get`,
    /// `resources/read` and every other request a client sends — counted from
    /// its dispatch; past it the operation is dropped and rolled back, and its
    /// client is answered a JSON-RPC internal error whose `data.reason` is
    /// `timeout`. `subscriptions/listen` has no end of its own and none.
    /// `None` ⇒ no deadline. Read from `<PREFIX>_MCP__OPERATION_TIMEOUT_SECS`,
    /// whole seconds from 1 to 3600 or `0` for none — refused outside, from the
    /// environment and from the pinned struct alike; defaults to 20 seconds,
    /// [`nest_rs_http::DEFAULT_UNIT_TIMEOUT`]. A deadline past the HTTP
    /// shutdown window, or none, is said at boot with a `warn`.
    #[config(bounds = nest_rs_http::UNIT_TIMEOUT_BOUNDS)]
    pub operation_timeout: Option<Duration>,
    /// Live sessions one mount holds at most. Past it, an `initialize` from a
    /// client older than `2026-07-28` is answered `503` with `Retry-After`
    /// until a session closes or idles out; a stateless request opens no
    /// session and is never refused for it. A session restored from a
    /// `SessionStore` counts like one created here. Read from
    /// `<PREFIX>_MCP__MAX_CONCURRENT_SESSIONS`, from 1 to 1 048 576 — refused
    /// outside, from the environment and from the pinned struct alike;
    /// defaults to 1 024.
    ///
    /// An idle session — its worker task, its channels and the handler `#[mcp]`
    /// builds for it — measured about 110 KiB of resident memory (4 000 sessions
    /// of a one-tool host, debug and release builds alike), so the default
    /// holds about 110 MiB at most. Two `initialize`s racing for the last place
    /// both pass the check that answers `503`; the second is then refused by
    /// the session manager, and answered the same `503`.
    #[config(bounds = SESSIONS)]
    pub max_concurrent_sessions: usize,
    /// How long a session may go without a message before it is closed, its
    /// place under [`max_concurrent_sessions`](Self::max_concurrent_sessions)
    /// freed; its client then gets `404` and opens a new one. Read from
    /// `<PREFIX>_MCP__SESSION_IDLE_TIMEOUT_SECS`, whole seconds from 1 to 3600,
    /// never off — refused outside, from the environment and from the pinned
    /// struct alike; defaults to 300 seconds.
    #[config(bounds = SESSION_IDLE_TIMEOUT)]
    pub session_idle_timeout: Duration,
}

impl Default for McpConfig {
    fn default() -> Self {
        // rmcp's own defaults, loopback allowlist included.
        Self {
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
            legacy_session_mode: true,
            json_response: false,
            sse_keep_alive: Some(nest_rs_http::DEFAULT_SSE_KEEP_ALIVE),
            sse_retry: Some(Duration::from_secs(3)),
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            stateless_protocol_metadata_required: false,
            operation_timeout: Some(nest_rs_http::DEFAULT_UNIT_TIMEOUT),
            max_concurrent_sessions: DEFAULT_MAX_CONCURRENT_SESSIONS,
            session_idle_timeout: DEFAULT_SESSION_IDLE_TIMEOUT,
        }
    }
}

impl McpConfig {
    /// Translate into the SDK's config; the mount fills in `session_store` and
    /// `cancellation_token`.
    pub(crate) fn to_server_config(&self) -> StreamableHttpServerConfig {
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(self.sse_keep_alive)
            .with_sse_retry(self.sse_retry)
            .with_legacy_session_mode(self.legacy_session_mode)
            .with_json_response(self.json_response)
            .with_allowed_hosts(self.allowed_hosts.clone())
            // `allowed_origins` stays empty: the endpoint checks `Origin` before rmcp.
            .with_max_request_body_bytes(self.max_request_body_bytes)
            .with_stateless_protocol_metadata_required(self.stateless_protocol_metadata_required)
    }

    /// The SDK's per-session config: the idle timeout, the pinned `initialize`
    /// wait, and the configured retry — rmcp primes a session's streams from
    /// here, never from the server config.
    pub(crate) fn to_session_config(&self) -> SessionConfig {
        // `#[non_exhaustive]`: rmcp's default, overwritten field by field.
        let mut session = SessionConfig::default();
        session.keep_alive = Some(self.session_idle_timeout);
        session.init_timeout = Some(SESSION_INIT_TIMEOUT);
        session.sse_retry = self.sse_retry;
        session
    }

    /// One boot `warn` for the endpoint at `path` when
    /// [`operation_timeout`](Self::operation_timeout) is off or outlasts the
    /// HTTP shutdown `window`: an operation admitted just before a stop can
    /// then be cut unanswered. Not a refusal — a deployment may accept it
    /// knowingly.
    pub(crate) fn warn_when_operations_outlast_the_window(&self, window: Duration, path: &str) {
        if self
            .operation_timeout
            .is_some_and(|timeout| timeout <= window)
        {
            return;
        }
        tracing::warn!(
            target: crate::TARGET,
            path,
            operation_timeout_secs = self.operation_timeout.map_or(0, |timeout| timeout.as_secs()),
            shutdown_timeout_secs = window.as_secs(),
            "the operation timeout outlasts the shutdown window: an operation admitted just \
             before a stop can be cut unanswered",
        );
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::{Config, ConfigService};

    use super::*;

    /// A stream's keep-alive and an operation's deadline default to the values
    /// the HTTP transport exports, never a copy of them.
    #[test]
    fn the_defaults_are_the_transports_own() {
        let d = McpConfig::default();
        assert_eq!(d.sse_keep_alive, Some(nest_rs_http::DEFAULT_SSE_KEEP_ALIVE));
        assert_eq!(
            d.operation_timeout,
            Some(nest_rs_http::DEFAULT_UNIT_TIMEOUT)
        );
    }

    /// One byte to a gibibyte, from the environment and from a pin, naming the
    /// variable and its range and never the value it was given.
    #[test]
    fn the_request_body_cap_is_held_to_its_range_from_either_side() {
        let var = nest_rs_config::var_name("mcp", "MAX_REQUEST_BODY_BYTES");
        for (raw, said) in [
            ("0", "must be at least 1 — "),
            ("1073741825", "must be at most 1073741824 — "),
        ] {
            let refused = McpConfig::from_env(
                &ConfigService::with_vars("mcp", [("MAX_REQUEST_BODY_BYTES", raw)]),
                McpConfig::default(),
            )
            .expect_err("outside the range")
            .to_string();
            assert!(
                refused.contains(&var) && refused.contains(said),
                "{refused}"
            );
            assert!(!refused.contains("1073741825"), "{refused}");
        }
        for (pinned, said) in [
            (0, "below the 1 it must be at least"),
            ((1 << 30) + 1, "above the 1073741824 it must be at most"),
        ] {
            let refused = McpConfig::from_env(
                &ConfigService::with_vars("mcp", []),
                McpConfig {
                    max_request_body_bytes: pinned,
                    ..McpConfig::default()
                },
            )
            .expect_err("a pin outside the range")
            .to_string();
            assert!(
                refused.contains(&var)
                    && refused.contains("`McpConfig::max_request_body_bytes` set in code")
                    && refused.contains(said),
                "{refused}"
            );
        }
    }

    #[test]
    fn defaults_keep_the_sdk_loopback_allowlist() {
        // Widening it opens every `#[mcp]` mount to DNS rebinding.
        assert_eq!(
            McpConfig::default().allowed_hosts,
            ["localhost", "127.0.0.1", "::1"],
        );
    }

    #[test]
    fn no_origin_knob_lives_on_the_mcp_config() {
        let rendered = format!("{:?}", McpConfig::default());
        assert!(
            !rendered.contains("allowed_origins"),
            "origin belongs to {}: {rendered}",
            nest_rs_config::var_name("http", "CORS_ORIGINS"),
        );
    }

    #[test]
    fn env_overlays_the_pinned_base_per_field() {
        let pinned = McpConfig {
            allowed_hosts: vec!["mcp.example.com".into()],
            ..McpConfig::default()
        };
        let cfg = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("JSON_RESPONSE", "true")]),
            pinned,
        )
        .expect("no error");

        assert!(cfg.json_response, "env wins on the field it sets");
        assert_eq!(
            cfg.allowed_hosts,
            ["mcp.example.com"],
            "a field the env does not set keeps the pinned value",
        );
    }

    #[test]
    fn allowlists_parse_as_comma_separated_lists() {
        let cfg = McpConfig::from_env(
            &ConfigService::with_vars(
                "mcp",
                [("ALLOWED_HOSTS", "mcp.example.com, mcp.example.com:8443")],
            ),
            McpConfig::default(),
        )
        .expect("no error");

        assert_eq!(
            cfg.allowed_hosts,
            ["mcp.example.com", "mcp.example.com:8443"]
        );
    }

    #[test]
    fn zero_seconds_turns_an_sse_duration_off() {
        let cfg = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("SSE_KEEP_ALIVE_SECS", "0")]),
            McpConfig::default(),
        )
        .expect("no error");

        assert_eq!(cfg.sse_keep_alive, None);
        assert_eq!(cfg.sse_retry, Some(Duration::from_secs(3)));
    }

    #[test]
    fn an_sse_duration_past_an_hour_is_refused_from_either_side() {
        for key in ["SSE_KEEP_ALIVE_SECS", "SSE_RETRY_SECS"] {
            let refused = McpConfig::from_env(
                &ConfigService::with_vars("mcp", [(key, "3601")]),
                McpConfig::default(),
            )
            .expect_err("past an hour")
            .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("mcp", key))
                    && refused.contains("must be at most 3600 seconds, or 0 to turn it off"),
                "{refused}"
            );
        }
        let pinned = McpConfig::from_env(
            &ConfigService::with_vars("mcp", []),
            McpConfig {
                sse_retry: Some(Duration::from_secs(7200)),
                ..McpConfig::default()
            },
        )
        .expect_err("a pinned retry past an hour")
        .to_string();
        assert!(
            pinned.contains("above the 3600s it must be at most"),
            "{pinned}"
        );
    }

    #[test]
    fn a_zero_operation_timeout_reads_as_none() {
        let cfg = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("OPERATION_TIMEOUT_SECS", "0")]),
            McpConfig::default(),
        )
        .expect("no error");
        assert_eq!(cfg.operation_timeout, None, "`0` is the off sentinel");
    }

    #[test]
    fn the_operation_timeout_reads_whole_seconds_over_a_pinned_base() {
        let pinned = McpConfig {
            operation_timeout: Some(Duration::from_secs(5)),
            ..McpConfig::default()
        };
        let kept = McpConfig::from_env(&ConfigService::with_vars("mcp", []), pinned.clone())
            .expect("no error");
        assert_eq!(kept.operation_timeout, Some(Duration::from_secs(5)));
        let read = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("OPERATION_TIMEOUT_SECS", "45")]),
            pinned,
        )
        .expect("no error");
        assert_eq!(read.operation_timeout, Some(Duration::from_secs(45)));
    }

    #[test]
    fn an_operation_timeout_outside_its_range_is_refused_naming_its_variable() {
        let var = nest_rs_config::var_name("mcp", "OPERATION_TIMEOUT_SECS");
        let refused = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("OPERATION_TIMEOUT_SECS", "3601")]),
            McpConfig::default(),
        )
        .expect_err("past an hour")
        .to_string();
        assert!(
            refused.contains(&var)
                && refused.contains("must be at most 3600 seconds, or 0 to turn it off"),
            "{refused}"
        );
    }

    /// The "default is ordered" half of the deadline rule, as HTTP's request
    /// timeout holds it.
    #[test]
    fn the_default_operation_timeout_fits_the_default_shutdown_window() {
        let timeout = McpConfig::default()
            .operation_timeout
            .expect("an operation is bounded by default");
        let window = nest_rs_http::HttpConfig::default().shutdown_timeout;
        assert!(
            timeout <= window,
            "{timeout:?} outlasts the {window:?} window"
        );
    }

    #[test]
    fn unset_session_keys_keep_a_thousand_sessions_and_five_idle_minutes() {
        let cfg = McpConfig::from_env(&ConfigService::with_vars("mcp", []), McpConfig::default())
            .expect("no error");
        assert_eq!(cfg.max_concurrent_sessions, 1024);
        assert_eq!(cfg.session_idle_timeout, Duration::from_secs(300));
    }

    #[test]
    fn the_session_keys_read_over_a_pinned_base() {
        let cfg = McpConfig::from_env(
            &ConfigService::with_vars(
                "mcp",
                [
                    ("MAX_CONCURRENT_SESSIONS", "64"),
                    ("SESSION_IDLE_TIMEOUT_SECS", "30"),
                ],
            ),
            McpConfig {
                max_concurrent_sessions: 8,
                session_idle_timeout: Duration::from_secs(10),
                ..McpConfig::default()
            },
        )
        .expect("no error");
        assert_eq!(cfg.max_concurrent_sessions, 64);
        assert_eq!(cfg.session_idle_timeout, Duration::from_secs(30));
    }

    /// `0` is no off switch: a cap of none would refuse every `initialize`.
    #[test]
    fn a_session_cap_of_none_is_refused_naming_its_variable() {
        let var = nest_rs_config::var_name("mcp", "MAX_CONCURRENT_SESSIONS");
        let refused = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("MAX_CONCURRENT_SESSIONS", "0")]),
            McpConfig::default(),
        )
        .expect_err("a cap of none")
        .to_string();
        assert!(
            refused.contains(&var) && refused.contains("must be at least 1"),
            "{refused}"
        );

        let pinned = McpConfig::from_env(
            &ConfigService::with_vars("mcp", []),
            McpConfig {
                max_concurrent_sessions: 0,
                ..McpConfig::default()
            },
        )
        .expect_err("a pinned cap of none")
        .to_string();
        assert!(
            pinned.contains(&var) && pinned.contains("McpConfig::max_concurrent_sessions"),
            "{pinned}"
        );
    }

    /// A cap past what a replica's memory holds bounds nothing, from either
    /// side.
    #[test]
    fn a_session_cap_past_a_million_is_refused_naming_its_variable() {
        let var = nest_rs_config::var_name("mcp", "MAX_CONCURRENT_SESSIONS");
        let refused = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("MAX_CONCURRENT_SESSIONS", "1048577")]),
            McpConfig::default(),
        )
        .expect_err("a cap no replica holds")
        .to_string();
        assert!(
            refused.contains(&var) && refused.contains("must be at most 1048576"),
            "{refused}"
        );
        let pinned = McpConfig::from_env(
            &ConfigService::with_vars("mcp", []),
            McpConfig {
                max_concurrent_sessions: usize::MAX,
                ..McpConfig::default()
            },
        )
        .expect_err("a pinned cap no replica holds")
        .to_string();
        assert!(
            pinned.contains(&var)
                && pinned.contains("`McpConfig::max_concurrent_sessions` set in code")
                && pinned.contains("above the 1048576 it must be at most"),
            "{pinned}"
        );
    }

    /// `0` is no off switch either: an idle session would hold its place forever.
    #[test]
    fn a_session_idle_timeout_outside_its_range_is_refused_naming_its_variable() {
        let var = nest_rs_config::var_name("mcp", "SESSION_IDLE_TIMEOUT_SECS");
        for (value, reason) in [
            ("0", "must be at least 1 second"),
            ("3601", "must be at most 3600 seconds"),
        ] {
            let refused = McpConfig::from_env(
                &ConfigService::with_vars("mcp", [("SESSION_IDLE_TIMEOUT_SECS", value)]),
                McpConfig::default(),
            )
            .expect_err("outside the range")
            .to_string();
            assert!(
                refused.contains(&var) && refused.contains(reason),
                "{value}: {refused}"
            );
        }
    }

    #[test]
    fn a_session_runs_on_the_configured_idle_timeout_retry_and_pinned_initialize_wait() {
        let session = McpConfig {
            session_idle_timeout: Duration::from_secs(42),
            sse_retry: None,
            ..McpConfig::default()
        }
        .to_session_config();
        assert_eq!(session.keep_alive, Some(Duration::from_secs(42)));
        assert_eq!(
            session.sse_retry, None,
            "the configured retry reaches sessions"
        );
        assert_eq!(session.init_timeout, Some(Duration::from_secs(60)));

        let retried = McpConfig::default().to_session_config();
        assert_eq!(retried.sse_retry, Some(Duration::from_secs(3)));
    }

    #[test]
    fn an_unparseable_value_is_a_boot_error() {
        let err = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("MAX_REQUEST_BODY_BYTES", "huge")]),
            McpConfig::default(),
        )
        .expect_err("a set-but-unparseable value fails boot");

        assert!(
            format!("{err}").contains("MAX_REQUEST_BODY_BYTES"),
            "the error names the offending key: {err}",
        );
    }
}

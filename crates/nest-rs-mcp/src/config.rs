//! [`McpConfig`] — streamable-HTTP server options for every `#[mcp]` mount.
//!
//! [`allowed_hosts`](McpConfig::allowed_hosts) is a security control: rmcp checks
//! the `Host` header against it to stop DNS rebinding, which reaches exactly the
//! local server that configures nothing — hence the loopback default. The
//! `Origin` half is the HTTP transport's CORS policy
//! (`<PREFIX>_HTTP__CORS_ORIGINS`), so rmcp's `allowed_origins` stays empty.

use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;

/// rmcp's own default POST body ceiling.
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

/// The stream keep-alive's range, every server-sent stream's.
const SSE_KEEP_ALIVE: DurationBounds = DurationBounds::secs(
    "SSE_KEEP_ALIVE_SECS",
    "McpConfig::sse_keep_alive",
    nest_rs_http::SSE_KEEP_ALIVE_FLOOR,
    nest_rs_http::SSE_KEEP_ALIVE_CEILING,
);

/// The advertised reconnection delay's range.
const SSE_RETRY: DurationBounds = DurationBounds::secs(
    "SSE_RETRY_SECS",
    "McpConfig::sse_retry",
    Floor::UnitsOrOff(Bound {
        count: 1,
        why: "a client told to come back sooner reconnects at once, and every client a restart \
              dropped does it together",
    }),
    Bound {
        count: 60 * 60,
        why: "a client told to wait more than an hour before resuming a dropped stream has \
              abandoned the session it would resume",
    },
);

/// MCP streamable-HTTP options resolved at boot (namespace `mcp`).
#[config(namespace = "mcp")]
#[derive(Clone, Debug)]
pub struct McpConfig {
    /// Hostnames or `host:port` authorities accepted in the inbound `Host`
    /// header (anti-DNS-rebinding). Defaults to loopback only, so a public
    /// deployment **must** name itself — `<PREFIX>_MCP__ALLOWED_HOSTS=mcp.example.com,mcp.example.com:8443`.
    /// An empty list disables the check entirely and is reported at `warn` at
    /// mount time.
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
    /// for none; defaults to 15s.
    pub sse_keep_alive: Option<Duration>,
    /// `retry:` interval advertised on SSE priming events. `None` ⇒ none. Read
    /// from `<PREFIX>_MCP__SSE_RETRY_SECS`, whole seconds from 1 to 3600 or `0`
    /// for none; defaults to 3s.
    pub sse_retry: Option<Duration>,
    /// Cap on a single POST body, enforced while streaming (independent of
    /// `Content-Length`); over it the client gets `413`. Read from
    /// `<PREFIX>_MCP__MAX_REQUEST_BODY_BYTES`; defaults to 4 MiB.
    #[validate(range(min = 1, message = "must be at least 1 byte"))]
    pub max_request_body_bytes: usize,
    /// Require per-request protocol metadata (`MCP-Protocol-Version` and
    /// `_meta.io.modelcontextprotocol/protocolVersion`) on stateless request
    /// POSTs, per SEP-2243. Rejects clients negotiated below `2026-07-28`, so
    /// turn it on together with a handler that advertises only `2026-07-28`
    /// and later. Read from `<PREFIX>_MCP__STATELESS_PROTOCOL_METADATA_REQUIRED`;
    /// defaults to `false`.
    pub stateless_protocol_metadata_required: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        // rmcp's own defaults, loopback allowlist included.
        Self {
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
            legacy_session_mode: true,
            json_response: false,
            sse_keep_alive: Some(Duration::from_secs(15)),
            sse_retry: Some(Duration::from_secs(3)),
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            stateless_protocol_metadata_required: false,
        }
    }
}

impl McpConfig {
    /// Pin the `Host` allowlist in code — the deployment's own hostnames.
    pub fn with_allowed_hosts(
        mut self,
        hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.allowed_hosts = hosts.into_iter().map(Into::into).collect();
        self
    }

    /// Translate into the SDK's config; the mount fills in `session_store` and
    /// `cancellation_token`.
    pub(crate) fn to_server_config(&self) -> StreamableHttpServerConfig {
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(self.sse_keep_alive)
            .with_sse_retry(self.sse_retry)
            .with_legacy_session_mode(self.legacy_session_mode)
            .with_json_response(self.json_response)
            .with_allowed_hosts(self.allowed_hosts.clone())
            // `allowed_origins` stays empty: `Origin` is the transport's CORS policy.
            .with_max_request_body_bytes(self.max_request_body_bytes)
            .with_stateless_protocol_metadata_required(self.stateless_protocol_metadata_required)
    }
}

impl Config for McpConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            allowed_hosts: env.list("ALLOWED_HOSTS", base.allowed_hosts)?,
            legacy_session_mode: env.flag("LEGACY_SESSION_MODE", base.legacy_session_mode)?,
            json_response: env.flag("JSON_RESPONSE", base.json_response)?,
            sse_keep_alive: SSE_KEEP_ALIVE
                .read_optional(env, base.sse_keep_alive)?
                .map(|read| read.value),
            sse_retry: SSE_RETRY
                .read_optional(env, base.sse_retry)?
                .map(|read| read.value),
            max_request_body_bytes: env
                .parse::<usize>("MAX_REQUEST_BODY_BYTES")?
                .unwrap_or(base.max_request_body_bytes),
            stateless_protocol_metadata_required: env.flag(
                "STATELESS_PROTOCOL_METADATA_REQUIRED",
                base.stateless_protocol_metadata_required,
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let pinned = McpConfig::default().with_allowed_hosts(["mcp.example.com"]);
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

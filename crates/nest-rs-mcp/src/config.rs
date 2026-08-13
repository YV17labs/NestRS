//! [`McpConfig`] — streamable-HTTP server options for every `#[mcp]` mount.
//!
//! One of these fields is a **security control**, not a tuning knob.
//!
//! rmcp validates the inbound `Host` header against
//! [`allowed_hosts`](McpConfig::allowed_hosts) to stop **DNS rebinding**: a
//! page on an attacker's origin resolves its own hostname to `127.0.0.1` and
//! POSTs to a developer's locally running MCP server, which would otherwise
//! answer with the user's tools and data. The SDK therefore ships a
//! loopback-only allowlist — which means a server reached under a *real*
//! hostname answers `403` until the deployment names itself here.
//!
//! **That default is deliberate, and inverting it would invert the
//! protection.** DNS rebinding only reaches a host the victim's browser can
//! resolve to but the attacker cannot call directly — loopback and the local
//! network. So the vulnerable deployment is the developer's local server, which
//! is exactly the one that configures nothing; defaulting to "off" would
//! disarm the control in the only case the attack works, and leave it armed
//! only where it is least needed. A deployment that genuinely wants no
//! validation says so with an empty list.
//!
//! **The browser `Origin` half is not here** — it is the HTTP transport's CORS
//! policy, `NESTRS_HTTP__CORS_ORIGINS`. rmcp offers its own `allowed_origins`
//! and the framework deliberately leaves it empty: poem rejects a disallowed
//! `Origin` with `403` on *every* method (not just the preflight), the CORS
//! layer wraps the whole route tree so a `#[mcp]` self-mount inherits it
//! (`EdgePosture::Exempt` skips guards, never CORS), and the two checks share
//! their open doors exactly (an empty list is off; a request with no `Origin`
//! passes). Two knobs for one control is what the framework forbids, and the
//! transport-wide one is the survivor because it also covers `/graphql`, `/ws`
//! and every controller.
//!
//! Dual-path like every `nest-rs-*` config: settable via `NESTRS_MCP__*` env
//! vars **and** via the pinned struct passed to
//! [`McpModule::for_root`](crate::McpModule::for_root), composing per field.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Result, config};
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;

/// rmcp's own default POST body ceiling, restated so the framework's default is
/// readable here rather than inherited invisibly.
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

/// MCP streamable-HTTP options resolved at boot (namespace `mcp`). See the
/// module docs for why the host allowlist is a security control.
#[config(namespace = "mcp")]
#[derive(Clone, Debug)]
pub struct McpConfig {
    /// Hostnames or `host:port` authorities accepted in the inbound `Host`
    /// header (anti-DNS-rebinding). Defaults to loopback only, so a public
    /// deployment **must** name itself — `NESTRS_MCP__ALLOWED_HOSTS=mcp.example.com,mcp.example.com:8443`.
    /// An empty list disables the check entirely and is reported at `warn` at
    /// mount time.
    pub allowed_hosts: Vec<String>,
    /// Keep sessions alive for protocol versions older than `2026-07-28`. Per
    /// SEP-2567 the `2026-07-28` revision is always served statelessly, so this
    /// only affects legacy clients. Read from
    /// `NESTRS_MCP__LEGACY_SESSION_MODE`; defaults to `true`.
    pub legacy_session_mode: bool,
    /// Answer simple request/response operations with `application/json`
    /// instead of an SSE stream (the server still falls back to
    /// `text/event-stream` when a handler emits a notification first). Read
    /// from `NESTRS_MCP__JSON_RESPONSE`; defaults to `false`.
    pub json_response: bool,
    /// SSE keep-alive ping interval. `None` ⇒ no pings. Read from
    /// `NESTRS_MCP__SSE_KEEP_ALIVE_SECS` (`0` ⇒ none); defaults to 15s.
    pub sse_keep_alive: Option<Duration>,
    /// `retry:` interval advertised on SSE priming events. `None` ⇒ none. Read
    /// from `NESTRS_MCP__SSE_RETRY_SECS` (`0` ⇒ none); defaults to 3s.
    pub sse_retry: Option<Duration>,
    /// Cap on a single POST body, enforced while streaming (independent of
    /// `Content-Length`); over it the client gets `413`. Read from
    /// `NESTRS_MCP__MAX_REQUEST_BODY_BYTES`; defaults to 4 MiB.
    #[validate(range(min = 1, message = "must be at least 1 byte"))]
    pub max_request_body_bytes: usize,
    /// Require per-request protocol metadata (`MCP-Protocol-Version` and
    /// `_meta.io.modelcontextprotocol/protocolVersion`) on stateless request
    /// POSTs, per SEP-2243. Rejects clients negotiated below `2026-07-28`, so
    /// turn it on together with a handler that advertises only `2026-07-28`
    /// and later. Read from `NESTRS_MCP__STATELESS_PROTOCOL_METADATA_REQUIRED`;
    /// defaults to `false`.
    pub stateless_protocol_metadata_required: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        // Mirrors rmcp's own defaults, loopback allowlist included — the
        // framework does not widen the SDK's security posture behind the
        // developer's back.
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

    /// Translate into the SDK's own config. `session_store` and
    /// `cancellation_token` are left at their defaults here — the mount fills
    /// the store in from the container, and the token is runtime state, not
    /// configuration.
    pub(crate) fn to_server_config(&self) -> StreamableHttpServerConfig {
        // `StreamableHttpServerConfig` is `#[non_exhaustive]`; the builder
        // methods are the supported way in, and they keep this compiling when
        // rmcp grows a field.
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(self.sse_keep_alive)
            .with_sse_retry(self.sse_retry)
            .with_legacy_session_mode(self.legacy_session_mode)
            .with_json_response(self.json_response)
            .with_allowed_hosts(self.allowed_hosts.clone())
            // `allowed_origins` stays at rmcp's empty default on purpose: the
            // `Origin` control is the transport's CORS policy. See the module
            // docs.
            .with_max_request_body_bytes(self.max_request_body_bytes)
            .with_stateless_protocol_metadata_required(self.stateless_protocol_metadata_required)
    }
}

impl Config for McpConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            allowed_hosts: env.list("ALLOWED_HOSTS", base.allowed_hosts),
            legacy_session_mode: env.flag("LEGACY_SESSION_MODE", base.legacy_session_mode)?,
            json_response: env.flag("JSON_RESPONSE", base.json_response)?,
            sse_keep_alive: env.seconds("SSE_KEEP_ALIVE_SECS", base.sse_keep_alive)?,
            sse_retry: env.seconds("SSE_RETRY_SECS", base.sse_retry)?,
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
        // Widening this default would hand every `#[mcp]` mount a DNS-rebinding
        // exposure the SDK deliberately closes.
        assert_eq!(
            McpConfig::default().allowed_hosts,
            ["localhost", "127.0.0.1", "::1"],
        );
    }

    /// The `Origin` control has exactly one home, and it is not this config.
    /// A re-added `allowed_origins` would resurrect the second knob poem's
    /// CORS layer already owns for every transport.
    #[test]
    fn no_origin_knob_lives_on_the_mcp_config() {
        let rendered = format!("{:?}", McpConfig::default());
        assert!(
            !rendered.contains("allowed_origins"),
            "origin belongs to NESTRS_HTTP__CORS_ORIGINS: {rendered}",
        );
    }

    // The dual-path rule is framework-wide: a pinned `McpConfig` still takes
    // its overrides per field from `NESTRS_MCP__*`.
    #[test]
    fn env_overlays_the_pinned_base_per_field() {
        let pinned = McpConfig::default().with_allowed_hosts(["mcp.example.com"]);
        let cfg = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("NESTRS_MCP__JSON_RESPONSE", "true")]),
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
                [(
                    "NESTRS_MCP__ALLOWED_HOSTS",
                    "mcp.example.com, mcp.example.com:8443",
                )],
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
            &ConfigService::with_vars("mcp", [("NESTRS_MCP__SSE_KEEP_ALIVE_SECS", "0")]),
            McpConfig::default(),
        )
        .expect("no error");

        assert_eq!(cfg.sse_keep_alive, None);
        assert_eq!(cfg.sse_retry, Some(Duration::from_secs(3)));
    }

    #[test]
    fn an_unparseable_value_is_a_boot_error() {
        let err = McpConfig::from_env(
            &ConfigService::with_vars("mcp", [("NESTRS_MCP__MAX_REQUEST_BODY_BYTES", "huge")]),
            McpConfig::default(),
        )
        .expect_err("a set-but-unparseable value fails boot");

        assert!(
            format!("{err}").contains("MAX_REQUEST_BODY_BYTES"),
            "the error names the offending key: {err}",
        );
    }
}

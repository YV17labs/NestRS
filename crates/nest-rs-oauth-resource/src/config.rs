//! [`OAuthResourceConfig`] — who this resource server is, and which
//! authorization servers issue tokens for it (RFC 9728 §2).

use nest_rs_config::{Config, ConfigService, Namespaced, config};

use crate::metadata::ProtectedResourceMetadata;
use nest_rs_authn::AuthError;

/// RFC 9728 §2 default for `bearer_methods_supported`: the only form the framework reads.
const BEARER_METHOD_HEADER: &str = "header";
/// RFC 9728 §2's closed set for `bearer_methods_supported`, in the order §2
/// lists them — RFC 6750 §2.1, §2.2, §2.3.
const BEARER_METHODS_DEFINED: [&str; 3] = [BEARER_METHOD_HEADER, "body", "query"];

/// Identity of this deployment as an OAuth 2.1 protected resource (namespace
/// `oauth__resource`).
#[config(namespace = "oauth__resource")]
#[derive(Clone, Debug, Default)]
pub struct OAuthResourceConfig {
    /// The canonical URI clients name in their RFC 8707 `resource` parameter, and
    /// the `aud` tokens must carry: absolute, no fragment, no trailing slash. Required.
    pub resource: Option<String>,
    /// Issuers of the authorization servers that mint tokens for this resource; at
    /// least one is required (RFC 9728 §2).
    pub authorization_servers: Vec<String>,
    /// The minimal scope set, advertised in the metadata document and the
    /// `WWW-Authenticate` challenge; empty omits both.
    pub scopes_supported: Vec<String>,
    /// How a token may be presented (RFC 9728 §2); defaults to `header`, the only
    /// form this framework accepts.
    pub bearer_methods_supported: Vec<String>,
    /// Human-readable name for a consent screen.
    pub resource_name: Option<String>,
    /// URL of developer documentation for this resource.
    pub resource_documentation: Option<String>,
    /// RFC 9728 §2 `resource_policy_uri`: how the resource's data may be used.
    pub resource_policy_uri: Option<String>,
    /// RFC 9728 §2 `resource_tos_uri`: the terms of service.
    pub resource_tos_uri: Option<String>,
}

impl Config for OAuthResourceConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            resource: env.get("RESOURCE")?.or(base.resource),
            authorization_servers: env.list("AUTHORIZATION_SERVERS", base.authorization_servers)?,
            scopes_supported: env.list("SCOPES_SUPPORTED", base.scopes_supported)?,
            bearer_methods_supported: env
                .list("BEARER_METHODS_SUPPORTED", base.bearer_methods_supported)?,
            resource_name: env.get("RESOURCE_NAME")?.or(base.resource_name),
            resource_policy_uri: env.get("RESOURCE_POLICY_URI")?.or(base.resource_policy_uri),
            resource_tos_uri: env.get("RESOURCE_TOS_URI")?.or(base.resource_tos_uri),
            resource_documentation: env
                .get("RESOURCE_DOCUMENTATION")?
                .or(base.resource_documentation),
        })
    }
}

impl OAuthResourceConfig {
    /// Pin the canonical resource URI in code.
    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    /// Pin the authorization server issuer list in code.
    pub fn with_authorization_servers(
        mut self,
        servers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.authorization_servers = servers.into_iter().map(Into::into).collect();
        self
    }

    /// Pin the advertised scope set in code.
    pub fn with_scopes_supported(
        mut self,
        scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.scopes_supported = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Validate the deployment's identity and freeze it into the document the
    /// well-known endpoint serves.
    pub fn into_metadata(self) -> Result<ProtectedResourceMetadata, AuthError> {
        let resource = self.resource.unwrap_or_default();
        let resource = resource.trim();
        if resource.is_empty() {
            return Err(AuthError::Failed(format!(
                "{} must name this deployment's canonical URI \
                 (for example https://api.example.com)",
                nest_rs_config::spellings(Self::NAMESPACE, "RESOURCE"),
            )));
        }
        validate_canonical_uri(resource)?;

        let authorization_servers: Vec<String> = self
            .authorization_servers
            .into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if authorization_servers.is_empty() {
            return Err(AuthError::Failed(format!(
                "{} must list at least one issuer — RFC 9728 metadata without \
                 `authorization_servers` tells a client nothing",
                nest_rs_config::spellings(Self::NAMESPACE, "AUTHORIZATION_SERVERS"),
            )));
        }
        for issuer in &authorization_servers {
            validate_canonical_uri(issuer)?;
        }

        // `scope` is space-delimited: a space or a quote would split a scope or
        // break out of the quoted challenge parameter.
        for scope in &self.scopes_supported {
            if !is_scope_token(scope) {
                return Err(AuthError::Failed(format!(
                    "`{scope}` is not a valid OAuth scope token (RFC 6749 §3.3): the grammar \
                     is `1*( %x21 / %x23-5B / %x5D-7E )` — printable ASCII excluding space, \
                     `\"` and `\\`"
                )));
            }
        }

        // RFC 9728 §2's set is closed; an unvalidated value would publish a
        // promise the extractor refuses.
        let bearer_methods_supported = if self.bearer_methods_supported.is_empty() {
            vec![BEARER_METHOD_HEADER.to_owned()]
        } else {
            for method in &self.bearer_methods_supported {
                if !BEARER_METHODS_DEFINED.contains(&method.as_str()) {
                    return Err(AuthError::Failed(format!(
                        "`{method}` is not an RFC 9728 §2 bearer method: the defined values \
                         are {BEARER_METHODS_DEFINED:?}",
                    )));
                }
                // `bearer_token` reads only the `Authorization` header, and a client acts
                // on the document before reaching a handler: refused, not warned.
                if method != BEARER_METHOD_HEADER {
                    return Err(AuthError::Failed(format!(
                        "`{method}` is a defined RFC 9728 bearer method, but this framework \
                         reads a bearer token only from the `Authorization` header — \
                         advertising it would tell clients to use a form this server refuses",
                    )));
                }
            }
            self.bearer_methods_supported
        };

        Ok(ProtectedResourceMetadata::new(
            resource.to_owned(),
            authorization_servers,
            self.scopes_supported,
            bearer_methods_supported,
            crate::metadata::ResourceDescription {
                resource_name: self.resource_name,
                resource_documentation: self.resource_documentation,
                resource_policy_uri: self.resource_policy_uri,
                resource_tos_uri: self.resource_tos_uri,
            },
        ))
    }
}

/// RFC 6749 §3.3: `scope-token = 1*( %x21 / %x23-5B / %x5D-7E )` — printable
/// ASCII excluding space (`%x20`), `"` (`%x22`) and `\` (`%x5C`).
fn is_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E))
}

/// Whether an authority names loopback, the one place a plain-`http` resource
/// identifier is legitimate. Parsed per RFC 3986 §3.2: `[::1]` holds colons, and
/// `http://localhost:8080@evil.com`'s host is `evil.com`.
fn is_loopback_authority(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Userinfo is everything before the *last* `@`; the host follows it.
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host_port.strip_prefix('[') {
        // An IP-literal is bracketed, so the port is whatever follows the `]`.
        Some(rest) => match rest.split_once(']') {
            Some((inner, _)) => inner,
            None => return false,
        },
        None => host_port.split(':').next().unwrap_or(host_port),
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(addr) => addr.is_loopback(),
        Err(_) => false,
    }
}

/// The MCP authorization spec's canonical-URI rules: absolute, with a scheme, no
/// fragment; a trailing slash is legal but discouraged, so it only warns.
fn validate_canonical_uri(uri: &str) -> Result<(), AuthError> {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return Err(AuthError::Failed(format!(
            "`{uri}` is not a canonical resource URI: it has no scheme (expected \
             something like https://api.example.com)"
        )));
    };
    if scheme.is_empty() || rest.is_empty() {
        return Err(AuthError::Failed(format!(
            "`{uri}` is not a canonical resource URI: scheme and authority are both required"
        )));
    }
    // RFC 9728 §1.2: a resource identifier uses the https scheme; loopback is
    // spared for local development.
    if !scheme.eq_ignore_ascii_case("https") && !is_loopback_authority(rest) {
        return Err(AuthError::Failed(format!(
            "`{uri}` is not a canonical resource URI: RFC 9728 §1.2 requires the https scheme (http is accepted only on localhost/127.0.0.1/[::1] for local development)"
        )));
    }
    if uri.contains('#') {
        return Err(AuthError::Failed(format!(
            "`{uri}` is not a canonical resource URI: a fragment is not allowed"
        )));
    }
    // These end up inside a quoted `WWW-Authenticate` parameter.
    if uri
        .chars()
        .any(|c| c.is_ascii_control() || matches!(c, '"' | '\\' | ' '))
    {
        return Err(AuthError::Failed(format!(
            "`{uri}` is not a canonical resource URI: it contains a space, a quote or a \
             control character"
        )));
    }
    if uri.len() > 1 && uri.ends_with('/') {
        tracing::warn!(
            target: crate::TARGET,
            uri,
            "resource URI ends in a trailing slash; clients are told to prefer the form without one",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> OAuthResourceConfig {
        OAuthResourceConfig::default()
            .with_resource("https://api.example.com")
            .with_authorization_servers(["https://auth.example.com"])
    }

    #[test]
    fn a_complete_config_produces_a_document() {
        let meta = valid().into_metadata().expect("valid config");
        assert_eq!(meta.resource(), "https://api.example.com");
        assert_eq!(
            meta.bearer_methods_supported(),
            ["header"],
            "the only presentation form the framework reads",
        );
    }

    #[test]
    fn a_missing_resource_names_its_env_var() {
        let err = OAuthResourceConfig::default()
            .with_authorization_servers(["https://auth.example.com"])
            .into_metadata()
            .expect_err("no resource");
        assert!(
            err.to_string().contains("RESOURCE"),
            "the boot error must be actionable: {err}",
        );
    }

    #[test]
    fn an_empty_authorization_server_list_fails_boot() {
        let err = OAuthResourceConfig::default()
            .with_resource("https://api.example.com")
            .into_metadata()
            .expect_err("no authorization server");
        assert!(
            err.to_string().contains("AUTHORIZATION_SERVERS"),
            "got: {err}",
        );
    }

    #[test]
    fn a_scheme_less_resource_is_refused() {
        let err = valid()
            .with_resource("api.example.com")
            .into_metadata()
            .expect_err("no scheme");
        assert!(err.to_string().contains("no scheme"), "got: {err}");
    }

    #[test]
    fn a_fragment_in_the_resource_is_refused() {
        let err = valid()
            .with_resource("https://api.example.com#mcp")
            .into_metadata()
            .expect_err("fragment");
        assert!(err.to_string().contains("fragment"), "got: {err}");
    }

    #[test]
    fn a_malformed_issuer_is_refused_too() {
        let err = valid()
            .with_authorization_servers(["auth.example.com"])
            .into_metadata()
            .expect_err("issuer without a scheme");
        assert!(err.to_string().contains("no scheme"), "got: {err}");
    }

    #[test]
    fn a_scope_carrying_a_space_is_refused() {
        let err = valid()
            .with_scopes_supported(["posts read"])
            .into_metadata()
            .expect_err("scope with a space");
        assert!(err.to_string().contains("RFC 6749 §3.3"), "got: {err}");
    }

    #[test]
    fn a_quote_in_the_resource_cannot_escape_the_challenge_parameter() {
        let err = valid()
            .with_resource("https://api.example.com/\"")
            .into_metadata()
            .expect_err("quote in the resource");
        assert!(err.to_string().contains("quote"), "got: {err}");
    }

    #[test]
    fn env_overlays_the_pinned_base_per_field() {
        let cfg = OAuthResourceConfig::from_env(
            &ConfigService::with_vars(
                OAuthResourceConfig::NAMESPACE,
                [("SCOPES_SUPPORTED", "posts:read, posts:write")],
            ),
            valid(),
        )
        .expect("no error");

        assert_eq!(cfg.scopes_supported, ["posts:read", "posts:write"]);
        assert_eq!(
            cfg.resource.as_deref(),
            Some("https://api.example.com"),
            "a field the env does not set keeps the pinned value",
        );
    }
    /// A trailing slash is legal, so it is reported, never refused.
    #[test]
    fn a_trailing_slash_is_accepted_and_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        valid()
            .with_resource("https://api.example.com/")
            .into_metadata()
            .expect("a trailing slash is discouraged, never refused");

        let event = logs
            .find(
                crate::TARGET,
                "resource URI ends in a trailing slash; clients are told to prefer the \
                 form without one",
            )
            .into_iter()
            .next()
            .expect("the discouraged form reports itself");
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("uri").as_deref(),
            Some("https://api.example.com/"),
            "and it quotes the URI, so an operator running several resources knows \
             which one to fix: {event:?}",
        );
    }

    #[test]
    fn the_canonical_form_is_silent() {
        let logs = nest_rs_testing::LogCapture::install();
        valid().into_metadata().expect("canonical");
        logs.expect_none(
            crate::TARGET,
            "resource URI ends in a trailing slash; clients are told to prefer the \
             form without one",
        );
    }
}

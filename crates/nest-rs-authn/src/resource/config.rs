//! [`ProtectedResourceConfig`] — who this resource server is, and which
//! authorization servers issue tokens for it (RFC 9728 §2).

use nest_rs_config::{Config, ConfigService, config, var_name};

use crate::error::AuthError;
use crate::resource::metadata::ProtectedResourceMetadata;

/// RFC 9728 §2 default for `bearer_methods_supported`: the framework only ever
/// reads `Authorization: Bearer`, and the MCP spec forbids the query-string
/// form outright.
const BEARER_METHOD_HEADER: &str = "header";

/// Identity of this deployment as an OAuth 2.1 protected resource (namespace
/// `authn`). Dual-path like every `nest-rs-*` config: `NESTRS_AUTHN__*` env
/// vars over the base pinned in
/// [`ProtectedResourceModule::for_root`](crate::ProtectedResourceModule::for_root),
/// composing per field.
#[config(namespace = "authn")]
#[derive(Clone, Debug, Default)]
pub struct ProtectedResourceConfig {
    /// The canonical URI clients name in their RFC 8707 `resource` parameter,
    /// and the value tokens must carry as `aud`. Absolute, no fragment, no
    /// trailing slash — `https://api.example.com` or
    /// `https://api.example.com/mcp`. Read from `NESTRS_AUTHN__RESOURCE`;
    /// **required** — the module fails boot without it.
    pub resource: Option<String>,
    /// Issuer identifiers of the authorization servers that mint tokens for
    /// this resource. Read from `NESTRS_AUTHN__AUTHORIZATION_SERVERS`;
    /// **at least one is required** (RFC 9728 §2, restated as a MUST by the
    /// MCP authorization spec).
    pub authorization_servers: Vec<String>,
    /// The minimal scope set for basic functionality, advertised in the
    /// metadata document and echoed in the `WWW-Authenticate` challenge so a
    /// client knows what to ask for. Read from
    /// `NESTRS_AUTHN__SCOPES_SUPPORTED`; empty omits both.
    pub scopes_supported: Vec<String>,
    /// How a token may be presented (RFC 9728 §2). Read from
    /// `NESTRS_AUTHN__BEARER_METHODS_SUPPORTED`; defaults to `header` alone,
    /// which is the only form this framework accepts.
    pub bearer_methods_supported: Vec<String>,
    /// Human-readable name for a consent screen. Read from
    /// `NESTRS_AUTHN__RESOURCE_NAME`.
    pub resource_name: Option<String>,
    /// URL of developer documentation for this resource. Read from
    /// `NESTRS_AUTHN__RESOURCE_DOCUMENTATION`.
    pub resource_documentation: Option<String>,
}

impl Config for ProtectedResourceConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            resource: env.get("RESOURCE").or(base.resource),
            authorization_servers: env.list("AUTHORIZATION_SERVERS", base.authorization_servers),
            scopes_supported: env.list("SCOPES_SUPPORTED", base.scopes_supported),
            bearer_methods_supported: env
                .list("BEARER_METHODS_SUPPORTED", base.bearer_methods_supported),
            resource_name: env.get("RESOURCE_NAME").or(base.resource_name),
            resource_documentation: env
                .get("RESOURCE_DOCUMENTATION")
                .or(base.resource_documentation),
        })
    }
}

impl ProtectedResourceConfig {
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
    ///
    /// Every check here is a boot failure rather than a runtime surprise: a
    /// resource server that cannot name itself canonically cannot bind an
    /// audience, and a client that trusts a malformed `resource` would request
    /// a token for something else.
    pub fn into_metadata(self) -> Result<ProtectedResourceMetadata, AuthError> {
        let resource = self.resource.unwrap_or_default();
        let resource = resource.trim();
        if resource.is_empty() {
            return Err(AuthError::Failed(format!(
                "{} must name this deployment's canonical URI \
                 (for example https://api.example.com)",
                var_name("authn", "RESOURCE"),
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
                var_name("authn", "AUTHORIZATION_SERVERS"),
            )));
        }
        for issuer in &authorization_servers {
            validate_canonical_uri(issuer)?;
        }

        // `scope` is a space-delimited list in both the metadata document and
        // the challenge, so a scope carrying a space or a quote would silently
        // become two scopes — or break out of the quoted parameter.
        for scope in &self.scopes_supported {
            if scope.is_empty()
                || scope
                    .chars()
                    .any(|c| c.is_ascii_control() || matches!(c, '"' | '\\' | ' '))
            {
                return Err(AuthError::Failed(format!(
                    "`{scope}` is not a valid OAuth scope token (RFC 6749 §3.3): scopes are \
                     space-delimited and carry no quotes or control characters"
                )));
            }
        }

        let bearer_methods_supported = if self.bearer_methods_supported.is_empty() {
            vec![BEARER_METHOD_HEADER.to_owned()]
        } else {
            self.bearer_methods_supported
        };

        Ok(ProtectedResourceMetadata::new(
            resource.to_owned(),
            authorization_servers,
            self.scopes_supported,
            bearer_methods_supported,
            self.resource_name,
            self.resource_documentation,
        ))
    }
}

/// The canonical-URI rules the MCP authorization spec spells out: absolute,
/// with a scheme, and no fragment. A trailing slash is legal but discouraged,
/// so it is a `warn` rather than a refusal — the deployment may genuinely mean
/// it.
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
    if uri.contains('#') {
        return Err(AuthError::Failed(format!(
            "`{uri}` is not a canonical resource URI: a fragment is not allowed"
        )));
    }
    // These end up inside a quoted `WWW-Authenticate` parameter. RFC 3986
    // forbids them in a URI anyway, so refusing at boot costs nothing and
    // removes any question of a value escaping its quotes.
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
            target: "nest_rs::authn",
            uri,
            "resource URI ends in a trailing slash; clients are told to prefer the form without one",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> ProtectedResourceConfig {
        ProtectedResourceConfig::default()
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
        let err = ProtectedResourceConfig::default()
            .with_authorization_servers(["https://auth.example.com"])
            .into_metadata()
            .expect_err("no resource");
        assert!(
            err.to_string().contains("NESTRS_AUTHN__RESOURCE"),
            "the boot error must be actionable: {err}",
        );
    }

    #[test]
    fn an_empty_authorization_server_list_fails_boot() {
        // RFC 9728 metadata whose `authorization_servers` is empty is a
        // document that satisfies the letter of discovery and none of its
        // purpose — the client learns nothing and cannot obtain a token.
        let err = ProtectedResourceConfig::default()
            .with_resource("https://api.example.com")
            .into_metadata()
            .expect_err("no authorization server");
        assert!(
            err.to_string()
                .contains("NESTRS_AUTHN__AUTHORIZATION_SERVERS"),
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
        // The issuer is what the client builds its AS-metadata URL from; a
        // scheme-less value would send it probing a relative path.
        let err = valid()
            .with_authorization_servers(["auth.example.com"])
            .into_metadata()
            .expect_err("issuer without a scheme");
        assert!(err.to_string().contains("no scheme"), "got: {err}");
    }

    #[test]
    fn a_scope_carrying_a_space_is_refused() {
        // `scope` is space-delimited on the wire: accepting this would publish
        // two scopes the deployment never wrote.
        let err = valid()
            .with_scopes_supported(["posts read"])
            .into_metadata()
            .expect_err("scope with a space");
        assert!(err.to_string().contains("space-delimited"), "got: {err}");
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
        let cfg = ProtectedResourceConfig::from_env(
            &ConfigService::with_vars(
                "authn",
                [("NESTRS_AUTHN__SCOPES_SUPPORTED", "posts:read, posts:write")],
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
    /// A trailing slash is *legal*, so this cannot be a boot failure — which is
    /// why the warning has to be right: it is the only thing a deployment whose
    /// clients compare identifiers byte-for-byte will ever see.
    #[test]
    fn a_trailing_slash_is_accepted_and_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        valid()
            .with_resource("https://api.example.com/")
            .into_metadata()
            .expect("a trailing slash is discouraged, never refused");

        let event = logs
            .find(
                "nest_rs::authn",
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

    /// The other half, and the reason the first is worth pinning: a warning that
    /// also fires on the good shape teaches operators to filter the target out.
    #[test]
    fn the_canonical_form_is_silent() {
        let logs = nest_rs_testing::LogCapture::install();
        valid().into_metadata().expect("canonical");
        assert!(
            logs.find(
                "nest_rs::authn",
                "resource URI ends in a trailing slash; clients are told to prefer the \
                 form without one",
            )
            .is_empty(),
        );
    }
}

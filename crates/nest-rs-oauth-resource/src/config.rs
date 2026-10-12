//! [`OAuthResourceConfig`] — who this resource server is, and which
//! authorization servers issue tokens for it (RFC 9728 §2).

use nest_rs_config::{Config, ConfigService, Environment, Namespaced, config};

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
        self.metadata_in(Environment::from_env())
    }

    /// [`into_metadata`](Self::into_metadata) under `environment`, the profile
    /// that decides whether plain `http` on loopback is accepted.
    fn metadata_in(self, environment: Environment) -> Result<ProtectedResourceMetadata, AuthError> {
        let resource = self.resource.unwrap_or_default();
        let resource = resource.trim();
        let resource_setting = nest_rs_config::spellings(Self::NAMESPACE, "RESOURCE");
        if resource.is_empty() {
            return Err(AuthError::Failed(format!(
                "{resource_setting} must name this deployment's canonical URI \
                 (for example https://api.example.com)",
            )));
        }
        validate_canonical_uri(resource, &resource_setting, environment)?;

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
        for (index, issuer) in authorization_servers.iter().enumerate() {
            validate_canonical_uri(issuer, &entry(index, "AUTHORIZATION_SERVERS"), environment)?;
        }

        // `scope` is space-delimited: a space or a quote would split a scope or
        // break out of the quoted challenge parameter.
        for (index, scope) in self.scopes_supported.iter().enumerate() {
            if !is_scope_token(scope) {
                return Err(AuthError::Failed(format!(
                    "{} is not a valid OAuth scope token (RFC 6749 §3.3): the grammar is \
                     `1*( %x21 / %x23-5B / %x5D-7E )` — printable ASCII excluding space, `\"` \
                     and `\\`",
                    entry(index, "SCOPES_SUPPORTED"),
                )));
            }
        }

        // RFC 9728 §2's set is closed; an unvalidated value would publish a
        // promise the extractor refuses.
        let bearer_methods_supported = if self.bearer_methods_supported.is_empty() {
            vec![BEARER_METHOD_HEADER.to_owned()]
        } else {
            for (index, method) in self.bearer_methods_supported.iter().enumerate() {
                if !BEARER_METHODS_DEFINED.contains(&method.as_str()) {
                    return Err(AuthError::Failed(format!(
                        "{} is not an RFC 9728 §2 bearer method: the defined values are \
                         {BEARER_METHODS_DEFINED:?}",
                        entry(index, "BEARER_METHODS_SUPPORTED"),
                    )));
                }
                // `bearer_token` reads only the `Authorization` header, and a client acts
                // on the document before reaching a handler: refused, not warned.
                if method != BEARER_METHOD_HEADER {
                    return Err(AuthError::Failed(format!(
                        "{} is the RFC 9728 bearer method `{method}`, but this framework reads a \
                         bearer token only from the `Authorization` header — advertising it \
                         would tell clients to use a form this server refuses",
                        entry(index, "BEARER_METHODS_SUPPORTED"),
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

/// Entry `index` (from zero) of the list `key` sets, as a refusal names it: by
/// place, never by value.
fn entry(index: usize, key: &str) -> String {
    format!(
        "entry {} of {}",
        index + 1,
        nest_rs_config::spellings(OAuthResourceConfig::NAMESPACE, key)
    )
}

/// RFC 6749 §3.3: `scope-token = 1*( %x21 / %x23-5B / %x5D-7E )` — printable
/// ASCII excluding space (`%x20`), `"` (`%x22`) and `\` (`%x5C`).
fn is_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E))
}

/// The authority of `rest`, a URI after its `://`: everything up to its path,
/// query or fragment (RFC 3986 §3.2).
fn authority_of(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// Whether an authority holding no userinfo names loopback. `[::1]` holds
/// colons, so an IP-literal's port is whatever follows its `]`.
fn is_loopback_authority(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((inner, _)) => inner,
            None => return false,
        },
        None => authority.split(':').next().unwrap_or(authority),
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(addr) => addr.is_loopback(),
        Err(_) => false,
    }
}

/// The MCP authorization spec's canonical-URI rules: absolute, with a scheme,
/// no userinfo, no fragment, and `https` but on loopback outside a deployment;
/// a trailing slash is legal but discouraged, so it only warns. A refusal names
/// `setting` and never the URI, and a log line shows it through
/// [`endpoint_shown`](nest_rs_config::endpoint_shown).
fn validate_canonical_uri(
    uri: &str,
    setting: &str,
    environment: Environment,
) -> Result<(), AuthError> {
    let refused =
        |why: &str| AuthError::Failed(format!("{setting} is not a canonical resource URI: {why}"));
    let Some((scheme, rest)) = uri.split_once("://") else {
        return Err(refused(
            "it has no scheme (expected something like https://api.example.com)",
        ));
    };
    if scheme.is_empty() || rest.is_empty() {
        return Err(refused("scheme and authority are both required"));
    }
    let authority = authority_of(rest);
    // The document publishes it, and a credential there would be published too.
    if authority.contains('@') {
        return Err(refused(
            "it carries userinfo, which RFC 9110 §4.2.4 forbids a sender to generate in an \
             http or https URI — name the host alone",
        ));
    }
    let local = !environment.is_deployed() && is_loopback_authority(authority);
    if !scheme.eq_ignore_ascii_case("https") && !local {
        return Err(refused(&format!(
            "RFC 9728 §1.2 requires the https scheme; plain http is accepted only on localhost, \
             127.0.0.1 or [::1], in a declared development or test profile ({}, here {})",
            Environment::var_name(),
            environment.as_str(),
        )));
    }
    if uri.contains('#') {
        return Err(refused("a fragment is not allowed"));
    }
    // These end up inside a quoted `WWW-Authenticate` parameter.
    if uri
        .chars()
        .any(|c| c.is_ascii_control() || matches!(c, '"' | '\\' | ' '))
    {
        return Err(refused(
            "it contains a space, a quote or a control character",
        ));
    }
    if uri.len() > 1 && uri.ends_with('/') {
        tracing::warn!(
            target: crate::TARGET,
            uri = nest_rs_config::endpoint_shown(uri).as_str(),
            "resource URI ends in a trailing slash; clients are told to prefer the form without one",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> OAuthResourceConfig {
        OAuthResourceConfig {
            resource: Some("https://api.example.com".into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..Default::default()
        }
    }

    /// [`valid`] naming `resource`.
    fn resource(resource: &str) -> OAuthResourceConfig {
        OAuthResourceConfig {
            resource: Some(resource.into()),
            ..valid()
        }
    }

    /// [`valid`] trusting `issuers`.
    fn issuers(issuers: &[&str]) -> OAuthResourceConfig {
        OAuthResourceConfig {
            authorization_servers: issuers.iter().map(|&issuer| issuer.into()).collect(),
            ..valid()
        }
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
        let err = OAuthResourceConfig {
            resource: None,
            ..valid()
        }
        .into_metadata()
        .expect_err("no resource");
        assert!(
            err.to_string().contains("RESOURCE"),
            "the boot error must be actionable: {err}",
        );
    }

    #[test]
    fn an_empty_authorization_server_list_fails_boot() {
        let err = issuers(&[])
            .into_metadata()
            .expect_err("no authorization server");
        assert!(
            err.to_string().contains("AUTHORIZATION_SERVERS"),
            "got: {err}",
        );
    }

    #[test]
    fn a_scheme_less_resource_is_refused() {
        let err = resource("api.example.com")
            .into_metadata()
            .expect_err("no scheme");
        assert!(err.to_string().contains("no scheme"), "got: {err}");
    }

    #[test]
    fn a_fragment_in_the_resource_is_refused() {
        let err = resource("https://api.example.com#mcp")
            .into_metadata()
            .expect_err("fragment");
        assert!(err.to_string().contains("fragment"), "got: {err}");
    }

    #[test]
    fn a_malformed_issuer_is_refused_too() {
        let err = issuers(&["auth.example.com"])
            .into_metadata()
            .expect_err("issuer without a scheme");
        assert!(err.to_string().contains("no scheme"), "got: {err}");
    }

    #[test]
    fn a_scope_carrying_a_space_is_refused() {
        let err = OAuthResourceConfig {
            scopes_supported: vec!["posts read".into()],
            ..valid()
        }
        .into_metadata()
        .expect_err("scope with a space");
        assert!(err.to_string().contains("RFC 6749 §3.3"), "got: {err}");
    }

    /// A refused value is described by its setting and place, never quoted: a
    /// resource URI or an issuer may carry userinfo.
    #[test]
    fn a_refused_value_is_named_by_its_setting_and_never_quoted() {
        for (config, setting) in [
            (resource("http://user:s3cr3t@api.example.com"), "RESOURCE"),
            (
                issuers(&[
                    "https://auth.example.com",
                    "http://user:s3cr3t@auth.example.com",
                ]),
                "AUTHORIZATION_SERVERS",
            ),
            (
                OAuthResourceConfig {
                    scopes_supported: vec!["posts:read".into(), "s3cr3t scope".into()],
                    ..valid()
                },
                "SCOPES_SUPPORTED",
            ),
            (
                OAuthResourceConfig {
                    bearer_methods_supported: vec!["s3cr3t".into()],
                    ..valid()
                },
                "BEARER_METHODS_SUPPORTED",
            ),
        ] {
            let err = config.into_metadata().expect_err(setting).to_string();
            assert!(
                err.contains(&nest_rs_config::var_name(
                    OAuthResourceConfig::NAMESPACE,
                    setting
                )),
                "{err}"
            );
            assert!(!err.contains("s3cr3t"), "{err}");
        }
    }

    /// RFC 9110 §4.2.4: an `https` URI a sender generates carries no userinfo,
    /// and these are published. Refused in every profile, by setting and
    /// place, with neither half of the credential in the refusal or a log line.
    #[test]
    fn userinfo_in_a_resource_or_an_issuer_is_refused_and_never_shown() {
        for environment in [Environment::Development, Environment::Production] {
            for (config, setting) in [
                (
                    resource("https://admin:s3cr3t@api.example.com/"),
                    "RESOURCE",
                ),
                (
                    issuers(&["https://admin:s3cr3t@auth.example.com/"]),
                    "AUTHORIZATION_SERVERS",
                ),
                (resource("https://s3cr3t@api.example.com/"), "RESOURCE"),
            ] {
                let logs = nest_rs_testing::LogCapture::install();
                let refused = config
                    .metadata_in(environment)
                    .expect_err("userinfo is refused")
                    .to_string();
                assert!(refused.contains("userinfo"), "{refused}");
                assert!(
                    refused.contains(&nest_rs_config::var_name(
                        OAuthResourceConfig::NAMESPACE,
                        setting
                    )),
                    "{refused}"
                );
                for half in ["admin", "s3cr3t"] {
                    assert!(!refused.contains(half), "{refused}");
                    for event in logs.events() {
                        assert!(
                            !format!("{event:?}").contains(half),
                            "a log line carries the credential: {event:?}"
                        );
                    }
                }
            }
        }
    }

    /// RFC 9728 §1.2's https rule spares loopback in a declared development or
    /// test profile alone: a deployment speaks https, on loopback too.
    #[test]
    fn plain_http_on_loopback_is_accepted_outside_a_deployment_only() {
        for environment in [
            Environment::Development,
            Environment::Test,
            Environment::Staging,
            Environment::Production,
        ] {
            for uri in [
                "http://localhost:3003",
                "http://127.0.0.1",
                "http://[::1]:8080",
            ] {
                for (config, setting) in [
                    (resource(uri), "RESOURCE"),
                    (issuers(&[uri]), "AUTHORIZATION_SERVERS"),
                ] {
                    let read = config.metadata_in(environment);
                    match environment.is_deployed() {
                        false => {
                            read.expect("loopback http serves local development");
                        }
                        true => {
                            let refused = read.expect_err("a deployment speaks https").to_string();
                            assert!(
                                refused.contains(&nest_rs_config::var_name(
                                    OAuthResourceConfig::NAMESPACE,
                                    setting
                                )) && refused.contains("https"),
                                "{environment:?} {uri}: {refused}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Outside loopback, plain http is refused in every profile.
    #[test]
    fn plain_http_elsewhere_is_refused_in_every_profile() {
        for environment in [Environment::Development, Environment::Production] {
            for uri in [
                "http://api.example.com",
                "http://localhost:8080@evil.example",
            ] {
                resource(uri)
                    .metadata_in(environment)
                    .expect_err("plain http off loopback");
            }
        }
    }

    /// The process's own `<PREFIX>_ENV` decides, whichever it is.
    #[test]
    fn into_metadata_reads_the_profile_the_process_declares() {
        let loopback = resource("http://localhost:3003");
        assert_eq!(
            loopback.into_metadata().is_ok(),
            !Environment::from_env().is_deployed(),
        );
    }

    #[test]
    fn a_quote_in_the_resource_cannot_escape_the_challenge_parameter() {
        let err = resource("https://api.example.com/\"")
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

    /// A trailing slash is legal, so it is reported, never refused — the URI
    /// shown as `endpoint_shown` shows one, its query never.
    #[test]
    fn a_trailing_slash_is_accepted_and_reported() {
        for (uri, shown) in [
            ("https://api.example.com/", "https://api.example.com/"),
            (
                "https://api.example.com/tenants/?key=s3cr3t/",
                "https://api.example.com/tenants/",
            ),
        ] {
            let logs = nest_rs_testing::LogCapture::install();
            resource(uri)
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
                Some(shown),
                "it names the URI, so an operator running several resources knows which one \
                 to fix: {event:?}",
            );
        }
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

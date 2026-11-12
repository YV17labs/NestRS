//! The [`ResourceBinding`] [`OAuthResourceModule`] hands the token verifier:
//! the audience and the issuers RFC 9068 §4 has a resource server require,
//! read off the resource it advertises.
//!
//! [`OAuthResourceModule`]: crate::OAuthResourceModule

use nest_rs_authn::__private::ResourceBinding;
use nest_rs_authn::AuthnConfig;
use nest_rs_config::Namespaced;

use crate::config::OAuthResourceConfig;
use crate::metadata::ProtectedResourceMetadata;

/// What a token must name to be accepted here, run once at boot before the
/// verifier is built: the configured audience, else the resource (RFC 8707);
/// as issuer, the configured one, else any advertised authorization server
/// (RFC 9068 §4). A differing audience only warns: some authorization servers
/// mint an opaque audience by policy. Every setting is named, never its value:
/// the resource is shown only through
/// [`endpoint_shown`](nest_rs_config::endpoint_shown), which alters the very
/// form it is compared in.
pub(crate) fn bind(
    authn: &AuthnConfig,
    metadata: &ProtectedResourceMetadata,
) -> anyhow::Result<ResourceBinding> {
    let authn_variable = |key| nest_rs_config::var_name(AuthnConfig::NAMESPACE, key);
    let resource_variable = |key| nest_rs_config::var_name(OAuthResourceConfig::NAMESPACE, key);
    if authn.allow_any_audience {
        anyhow::bail!(
            "{} is set beside OAuthResourceModule, which binds every token to the resource \
             {} names: an audience-agnostic verifier would accept a token minted for another \
             service — remove it",
            authn_variable("ALLOW_ANY_AUDIENCE"),
            resource_variable("RESOURCE"),
        );
    }

    // `<PREFIX>_AUTHN__AUDIENCE=` in a `.env` reads as `Some("")`: no audience.
    let audience = match authn
        .audience
        .as_deref()
        .filter(|set| !set.trim().is_empty())
    {
        None => metadata.resource().to_owned(),
        Some(audience) => {
            if audience != metadata.resource() {
                tracing::warn!(
                    target: crate::TARGET,
                    audience_variable = authn_variable("AUDIENCE").as_str(),
                    resource_variable = resource_variable("RESOURCE").as_str(),
                    "token audience differs from the advertised resource identifier — a client \
                     following RFC 8707 will request `resource` and receive a token this server \
                     rejects",
                );
            }
            audience.to_owned()
        }
    };

    let servers = metadata.authorization_servers();
    let issuers = match authn.issuer.as_deref().filter(|set| !set.trim().is_empty()) {
        None => servers.to_vec(),
        Some(issuer) if servers.iter().any(|server| server == issuer) => vec![issuer.to_owned()],
        Some(_) => anyhow::bail!(
            "{} is not one of {}: RFC 9068 §4 accepts a token only when its `iss` exactly \
             matches an authorization server this resource advertises, so every token naming \
             it would be refused — set it to one of them, or unset it to accept each",
            authn_variable("ISSUER"),
            resource_variable("AUTHORIZATION_SERVERS"),
        ),
    };
    Ok(ResourceBinding { audience, issuers })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESOURCE: &str = "https://api.example.com";
    const SERVERS: [&str; 2] = ["https://auth.example.com", "https://login.example.com"];

    fn metadata() -> ProtectedResourceMetadata {
        OAuthResourceConfig {
            resource: Some(RESOURCE.into()),
            authorization_servers: SERVERS.iter().map(|&server| server.into()).collect(),
            ..Default::default()
        }
        .into_metadata()
        .expect("valid config")
    }

    fn authn(audience: Option<&str>, issuer: Option<&str>) -> AuthnConfig {
        AuthnConfig {
            audience: audience.map(str::to_owned),
            issuer: issuer.map(str::to_owned),
            ..AuthnConfig::default()
        }
    }

    fn refusal(authn: &AuthnConfig) -> String {
        match bind(authn, &metadata()) {
            Ok(binding) => panic!("bound to {binding:?}"),
            Err(refused) => refused.to_string(),
        }
    }

    #[test]
    fn an_unset_or_blank_audience_is_the_resource() {
        for audience in [None, Some(""), Some("  ")] {
            let binding = bind(&authn(audience, None), &metadata()).expect("bound");
            assert_eq!(binding.audience, RESOURCE, "{audience:?}");
        }
    }

    #[test]
    fn a_matching_audience_binds_and_says_nothing() {
        let logs = nest_rs_testing::LogCapture::install();
        let binding = bind(&authn(Some(RESOURCE), None), &metadata()).expect("bound");
        assert_eq!(binding.audience, RESOURCE);
        assert!(
            logs.events().is_empty(),
            "the correct configuration is not an event: {:#?}",
            logs.events(),
        );
    }

    /// A mismatch is a `warn`, not a refusal, and the set audience is the one
    /// required. It names the two settings, echoing neither.
    #[test]
    fn a_differing_audience_is_bound_and_names_the_two_settings_that_disagree() {
        let logs = nest_rs_testing::LogCapture::install();
        let binding = bind(&authn(Some("some-opaque-audience"), None), &metadata())
            .expect("a mismatch is tolerated, not refused");
        assert_eq!(binding.audience, "some-opaque-audience");

        let event = logs.expect_one(
            crate::TARGET,
            "token audience differs from the advertised resource identifier — a client \
             following RFC 8707 will request `resource` and receive a token this server rejects",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("audience_variable"),
            Some(nest_rs_config::var_name(AuthnConfig::NAMESPACE, "AUDIENCE")),
        );
        assert_eq!(
            event.field("resource_variable"),
            Some(nest_rs_config::var_name(
                OAuthResourceConfig::NAMESPACE,
                "RESOURCE"
            )),
        );
        for value in ["some-opaque-audience", "api.example.com"] {
            assert!(!format!("{event:?}").contains(value), "{event:?}");
        }
    }

    #[test]
    fn an_unset_issuer_accepts_every_advertised_server() {
        let binding = bind(&authn(None, None), &metadata()).expect("bound");
        assert_eq!(binding.issuers, SERVERS);
    }

    #[test]
    fn a_set_issuer_among_the_servers_is_the_one_accepted() {
        let binding = bind(&authn(None, Some(SERVERS[1])), &metadata()).expect("bound");
        assert_eq!(binding.issuers, [SERVERS[1]]);
    }

    /// RFC 9068 §4's match is exact: a trailing slash is another issuer.
    #[test]
    fn an_issuer_outside_the_servers_is_refused_naming_both_settings() {
        for issuer in ["nestrs-demo-auth", "https://auth.example.com/"] {
            let refused = refusal(&authn(None, Some(issuer)));
            assert!(
                refused.contains(&nest_rs_config::var_name(AuthnConfig::NAMESPACE, "ISSUER"))
                    && refused.contains(&nest_rs_config::var_name(
                        OAuthResourceConfig::NAMESPACE,
                        "AUTHORIZATION_SERVERS"
                    )),
                "{refused}"
            );
            assert!(!refused.contains("example.com"), "{refused}");
            assert!(!refused.contains("nestrs-demo-auth"), "{refused}");
        }
    }

    #[test]
    fn any_audience_is_refused_naming_both_settings() {
        let refused = refusal(&AuthnConfig {
            allow_any_audience: true,
            ..AuthnConfig::default()
        });
        assert!(
            refused.contains(&nest_rs_config::var_name(
                AuthnConfig::NAMESPACE,
                "ALLOW_ANY_AUDIENCE"
            )) && refused.contains(&nest_rs_config::var_name(
                OAuthResourceConfig::NAMESPACE,
                "RESOURCE"
            )),
            "{refused}"
        );
    }
}

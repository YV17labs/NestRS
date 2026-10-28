//! The OAuth2 `client_credentials` grant: authenticating a registered machine
//! client against a static registry, in constant time.

use aws_lc_rs::constant_time::verify_slices_are_equal;

use nest_rs_authn::AuthError;
use nest_rs_authn::PrincipalIdentity;
use nest_rs_core::{AmbientValue, Secret};

use crate::TokenError;

/// A machine client permitted to use the `client_credentials` grant, as loaded
/// from configuration. Generic over the principal payload `P` the app attaches
/// (deserialized from the `payload` field next to the credentials).
#[derive(Clone, Debug, serde::Deserialize)]
#[non_exhaustive]
pub struct RegisteredClient<P> {
    /// The client's public identifier, matched in constant time.
    pub client_id: String,
    /// The client's shared secret, matched in constant time.
    pub client_secret: Secret<String>,
    /// Scopes granted to this client, copied onto the authenticated principal.
    pub scopes: Vec<String>,
    /// The resources (RFC 8707) a token minted for this client may name as
    /// its audience, copied onto the authenticated principal; the one a
    /// request naming none is minted for when it lists exactly one.
    pub resources: Vec<String>,
    /// App-defined per-client data (tenant id, role set, …) carried onto the
    /// principal — nest-rs never inspects it.
    pub payload: P,
}

impl<P> RegisteredClient<P> {
    /// The client `client_id`, authenticated by `client_secret`, granted
    /// `scopes`, minted tokens for `resources` alone and carrying `payload` —
    /// as a registry pinned in code lists it.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
        resources: impl IntoIterator<Item = impl Into<String>>,
        payload: P,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: Secret::new(client_secret.into()),
            scopes: scopes.into_iter().map(Into::into).collect(),
            resources: resources.into_iter().map(Into::into).collect(),
            payload,
        }
    }
}

/// The principal established by authenticating a [`RegisteredClient`] at the
/// token endpoint (RFC 6749 §2.3.1). A client acting on its own behalf is its
/// own subject (RFC 9068 §2.2), so [`PrincipalIdentity::actor_id`] is its
/// `client_id`: the audit names it, and the access token an app issues it names
/// it as its `sub`. The `payload` carries whatever else the app logs and
/// authorizes on.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AuthenticatedClient<P> {
    /// The matched client's public identifier (RFC 6749 §2.2).
    pub client_id: String,
    /// The matched client's granted scopes — reported verbatim as this
    /// principal's [`PrincipalIdentity::scopes`].
    pub scopes: Vec<String>,
    /// The resources (RFC 8707) the matched client is registered for — what
    /// [`audience_for`](Self::audience_for) chooses among.
    pub resources: Vec<String>,
    /// The matched client's app-defined payload — what the app logs and
    /// authorizes on.
    pub payload: P,
}

impl<P> AuthenticatedClient<P> {
    /// The client `client_id`, authenticated, granted `scopes`, registered for
    /// `resources` and carrying `payload` — as [`authenticate_against_registry`]
    /// answers it.
    pub fn new(
        client_id: impl Into<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
        resources: impl IntoIterator<Item = impl Into<String>>,
        payload: P,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            scopes: scopes.into_iter().map(Into::into).collect(),
            resources: resources.into_iter().map(Into::into).collect(),
            payload,
        }
    }

    /// The audience of the token this client's request is answered with, read
    /// off its RFC 8707 §2 `resource` parameters
    /// ([`AccessTokenRequest::resource`](crate::AccessTokenRequest::resource)):
    /// the one it names, when it is registered for it exactly; when it names
    /// none, the one resource it is registered for. Anything else is
    /// [`TokenError::InvalidTarget`] — an unregistered resource, none among
    /// several, and two at once, since a token naming two audiences is a
    /// deputy of each at the other.
    pub fn audience_for(&self, requested: &[String]) -> Result<&str, TokenError> {
        let wanted = match (requested, self.resources.as_slice()) {
            ([], [only]) => only,
            ([one], _) => one,
            _ => return Err(TokenError::InvalidTarget),
        };
        self.resources
            .iter()
            .find(|registered| *registered == wanted)
            .map(String::as_str)
            .ok_or(TokenError::InvalidTarget)
    }
}

impl<P: Send + Sync + 'static> AmbientValue for AuthenticatedClient<P> {}

impl<P> PrincipalIdentity for AuthenticatedClient<P> {
    /// The client's own id: the subject of what it does for itself.
    fn actor_id(&self) -> Option<String> {
        Some(self.client_id.clone())
    }

    /// `Some`, always: an empty entry means *delegated nothing*, where `None` would
    /// read as not scope-aware and pass every scope rule.
    fn scopes(&self) -> Option<&[String]> {
        Some(&self.scopes)
    }
}

/// Authenticate a `client_id` + `client_secret` pair against a static registry,
/// returning the matching principal. Both comparisons run in **constant time**
/// (AWS-LC's `CRYPTO_memcmp`) and every entry is visited, so neither a valid `client_id` nor a
/// secret prefix is observable through a timing side-channel. Returns
/// [`AuthError::Failed`] with an opaque message when no entry matches.
pub fn authenticate_against_registry<P: Clone>(
    clients: &[RegisteredClient<P>],
    client_id: &str,
    client_secret: &str,
) -> Result<AuthenticatedClient<P>, AuthError> {
    let mut matched: Option<&RegisteredClient<P>> = None;
    for client in clients {
        let id_ok =
            verify_slices_are_equal(client.client_id.as_bytes(), client_id.as_bytes()).is_ok();
        let secret_ok = verify_slices_are_equal(
            client.client_secret.expose_secret().as_bytes(),
            client_secret.as_bytes(),
        )
        .is_ok();
        if id_ok & secret_ok && matched.is_none() {
            matched = Some(client);
        }
    }
    let client = matched.ok_or_else(|| AuthError::failed("invalid client credentials"))?;
    Ok(AuthenticatedClient {
        client_id: client.client_id.clone(),
        scopes: client.scopes.clone(),
        resources: client.resources.clone(),
        payload: client.payload.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const API: &str = "https://api.example.com";
    const MCP: &str = "https://mcp.example.com/mcp";

    fn client(id: &str, secret: &str, scopes: &[&str]) -> RegisteredClient<u32> {
        RegisteredClient::new(id, secret, scopes.iter().copied(), [API], 7)
    }

    fn registered_for(resources: &[&str]) -> AuthenticatedClient<u32> {
        AuthenticatedClient::new("ci", ["read"], resources.iter().copied(), 7)
    }

    fn requested(resources: &[&str]) -> Vec<String> {
        resources
            .iter()
            .map(|&resource| resource.to_owned())
            .collect()
    }

    #[test]
    fn a_requested_resource_the_client_is_registered_for_is_the_audience() {
        let client = registered_for(&[API, MCP]);
        assert_eq!(client.audience_for(&requested(&[MCP])).unwrap(), MCP);
        assert_eq!(client.audience_for(&requested(&[API])).unwrap(), API);
    }

    /// RFC 8707 §2: an omitted `resource` may be "a predefined default
    /// resource value" — the one a client registered for a single resource
    /// names; registered for several, nothing says which.
    #[test]
    fn an_omitted_resource_is_the_one_registered_or_refused_among_several() {
        assert_eq!(
            registered_for(&[API]).audience_for(&[]).unwrap(),
            API,
            "the one resource registered"
        );
        assert!(matches!(
            registered_for(&[API, MCP]).audience_for(&[]),
            Err(TokenError::InvalidTarget)
        ));
    }

    /// One token, one audience: a token for two resources is a deputy of each
    /// at the other, so RFC 8707 §2's `invalid_target` refuses the
    /// combination.
    #[test]
    fn a_resource_outside_the_registration_or_two_at_once_is_an_invalid_target() {
        let client = registered_for(&[API, MCP]);
        for resource in [
            requested(&["https://billing.example.com"]),
            requested(&["https://api.example.com/"]),
            requested(&[API, MCP]),
            requested(&[API, API]),
        ] {
            assert!(
                matches!(
                    client.audience_for(&resource),
                    Err(TokenError::InvalidTarget)
                ),
                "{resource:?}"
            );
        }
        assert!(matches!(
            registered_for(&[]).audience_for(&[]),
            Err(TokenError::InvalidTarget)
        ));
    }

    #[test]
    fn the_registry_hands_the_client_its_resources() {
        let registry = [client("ci", "s3cret", &["read"])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").unwrap();
        assert_eq!(auth.resources, [API]);
    }

    #[test]
    fn debug_redacts_the_client_secret() {
        let rendered = format!("{:?}", client("ci", "s3cret", &["read"]));
        assert!(
            !rendered.contains("s3cret"),
            "a registry `{{:?}}` must never print live secrets: {rendered}",
        );
        assert!(rendered.contains("ci"), "the public id stays: {rendered}");
    }

    #[test]
    fn authenticates_a_matching_pair_and_returns_the_payload() {
        let registry = [client("ci", "s3cret", &["read"])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").expect("auth");
        assert_eq!(auth.client_id, "ci");
        assert_eq!(auth.payload, 7);
        assert_eq!(auth.scopes, vec!["read".to_string()]);
    }

    #[test]
    fn rejects_a_wrong_secret_with_an_opaque_error() {
        let registry = [client("ci", "s3cret", &["read"])];
        let err = authenticate_against_registry(&registry, "ci", "wrong").expect_err("rejected");
        assert!(matches!(err, AuthError::Failed { .. }));
        assert_eq!(
            nest_rs_core::error_message(&err),
            "authentication failed: invalid client credentials"
        );
    }

    #[test]
    fn rejects_an_empty_registry() {
        let registry: [RegisteredClient<u32>; 0] = [];
        assert!(authenticate_against_registry(&registry, "any", "any").is_err());
    }

    #[test]
    fn a_credential_matches_only_byte_for_byte_whatever_its_length() {
        let registry = [client("ci", "s3cret", &["read"])];
        for (id, secret, matches) in [
            ("ci", "s3cret", true),
            ("ci", "s3creT", false),
            ("ci", "s3cre", false),
            ("ci", "s3crets", false),
            ("ci", "", false),
            ("cI", "s3cret", false),
            ("c", "s3cret", false),
            ("cii", "s3cret", false),
            ("ghost", "s3cret", false),
        ] {
            assert_eq!(
                authenticate_against_registry(&registry, id, secret).is_ok(),
                matches,
                "{id:?} / {secret:?}"
            );
        }
    }

    #[test]
    fn distinguishes_clients_sharing_a_prefix() {
        let registry = [
            client("ci", "s3cret-a", &["a"]),
            client("ci-prod", "s3cret-b", &["b"]),
        ];
        let auth = authenticate_against_registry(&registry, "ci-prod", "s3cret-b").unwrap();
        assert_eq!(auth.scopes, vec!["b".to_string()]);
    }

    #[test]
    fn a_machine_principal_is_its_own_subject() {
        let registry = [client("ci", "s3cret", &["read"])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").unwrap();
        assert_eq!(auth.actor_id().as_deref(), Some("ci"));
    }

    #[test]
    fn machine_principal_reports_the_scopes_the_registry_granted() {
        let registry = [client("ci", "s3cret", &["read"])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").unwrap();
        assert_eq!(auth.scopes(), Some(["read".to_string()].as_slice()));
    }

    #[test]
    fn a_client_granted_nothing_is_delegated_nothing_not_unrestricted() {
        let registry = [client("ci", "s3cret", &[])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").unwrap();
        assert_eq!(
            auth.scopes(),
            Some([].as_slice()),
            "an entry granting no scope withholds every scoped rule",
        );
    }
}

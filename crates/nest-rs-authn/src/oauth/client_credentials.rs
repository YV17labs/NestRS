//! The OAuth2 `client_credentials` grant: authenticating a registered machine
//! client against a static registry, in constant time.
//!
//! The framework owns the credential check and the principal shape; the app
//! supplies the per-client payload `P` (a tenant id, a role set, …) — nest-rs
//! never names it. Pairs with the Authorization Code [`client`](super::client)
//! flow: this is the machine-to-machine grant, that is the user-delegated one.

use subtle::ConstantTimeEq;

use crate::PrincipalIdentity;
use crate::error::AuthError;

/// A machine client permitted to use the `client_credentials` grant, as loaded
/// from configuration. Generic over the principal payload `P` the app attaches
/// (deserialized from the `payload` field next to the credentials).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegisteredClient<P> {
    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<String>,
    pub payload: P,
}

/// The principal established by authenticating a [`RegisteredClient`]. A machine
/// actor with no per-user identity, so [`PrincipalIdentity::actor_id`] is `None`
/// — the `payload` carries whatever the app logs and authorizes on.
#[derive(Debug, Clone)]
pub struct AuthenticatedClient<P> {
    pub payload: P,
    pub scopes: Vec<String>,
}

impl<P> PrincipalIdentity for AuthenticatedClient<P> {
    fn actor_id(&self) -> Option<String> {
        None
    }
}

/// Authenticate a `client_id` + `client_secret` pair against a static registry,
/// returning the matching principal. Both comparisons run in **constant time**
/// (`subtle`) and every entry is visited, so neither a valid `client_id` nor a
/// secret prefix is observable through a timing side-channel. Returns
/// [`AuthError::Failed`] with an opaque message when no entry matches.
pub fn authenticate_against_registry<P: Clone>(
    clients: &[RegisteredClient<P>],
    client_id: &str,
    client_secret: &str,
) -> Result<AuthenticatedClient<P>, AuthError> {
    let mut matched: Option<&RegisteredClient<P>> = None;
    for client in clients {
        let id_ok = client.client_id.as_bytes().ct_eq(client_id.as_bytes());
        let secret_ok = client
            .client_secret
            .as_bytes()
            .ct_eq(client_secret.as_bytes());
        if bool::from(id_ok & secret_ok) && matched.is_none() {
            matched = Some(client);
        }
    }
    let client = matched.ok_or_else(|| AuthError::Failed("invalid client credentials".into()))?;
    Ok(AuthenticatedClient {
        payload: client.payload.clone(),
        scopes: client.scopes.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: &str, secret: &str, scopes: &[&str]) -> RegisteredClient<u32> {
        RegisteredClient {
            client_id: id.into(),
            client_secret: secret.into(),
            scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
            payload: 7,
        }
    }

    #[test]
    fn authenticates_a_matching_pair_and_returns_the_payload() {
        let registry = [client("ci", "s3cret", &["read"])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").expect("auth");
        assert_eq!(auth.payload, 7);
        assert_eq!(auth.scopes, vec!["read".to_string()]);
    }

    #[test]
    fn rejects_a_wrong_secret_with_an_opaque_error() {
        let registry = [client("ci", "s3cret", &["read"])];
        let err = authenticate_against_registry(&registry, "ci", "wrong").expect_err("rejected");
        assert!(matches!(err, AuthError::Failed(_)));
        assert!(err.to_string().contains("invalid client credentials"));
    }

    #[test]
    fn rejects_an_unknown_client_id() {
        let registry = [client("ci", "s3cret", &["read"])];
        assert!(authenticate_against_registry(&registry, "ghost", "s3cret").is_err());
    }

    #[test]
    fn rejects_an_empty_registry() {
        let registry: [RegisteredClient<u32>; 0] = [];
        assert!(authenticate_against_registry(&registry, "any", "any").is_err());
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
    fn machine_principal_has_no_actor_id() {
        let registry = [client("ci", "s3cret", &["read"])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").unwrap();
        assert_eq!(auth.actor_id(), None);
    }
}

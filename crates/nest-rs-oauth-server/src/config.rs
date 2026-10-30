//! [`OAuthServerConfig`] — the clients registered at this authorization server
//! (RFC 6749 §2), under `<PREFIX>_OAUTH__SERVER__*`.

use std::collections::HashSet;

use nest_rs_config::{Config, ConfigService, config};
use serde::de::DeserializeOwned;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

use crate::registry::RegisteredClient;

/// The clients this authorization server registers (RFC 6749 §2), generic over
/// the payload `P` each carries onto its principal — the product's own data,
/// which the framework never reads.
///
/// `<PREFIX>_OAUTH__SERVER__CLIENTS` holds them as a JSON array of
/// [`RegisteredClient`]s (or `_FILE`, a path to one); a value that does not
/// parse fails the boot naming the variable, never quoting it. The boot also
/// refuses a registry that could serve no token — no client, a client
/// registered for no resource (RFC 8707: a token names its audience) — and one
/// that could serve the wrong one: a client id registered twice, a client with
/// no id, or a client whose secret is empty, which an empty `Basic` password
/// would match. Each refusal names the client by its id, never its secret.
///
/// ```
/// use nest_rs_oauth_server::{OAuthServerConfig, RegisteredClient};
///
/// let config = OAuthServerConfig {
///     clients: vec![RegisteredClient::new(
///         "billing",
///         "s3cret",
///         ["invoices:read"],
///         ["https://invoices.example.com"],
///         (),
///     )],
/// };
/// # use validator::Validate as _;
/// # config.validate().expect("a registry that serves");
/// ```
#[config(namespace = "oauth__server", validate = "manual", from_env = "manual")]
#[derive(Clone, Debug)]
pub struct OAuthServerConfig<P> {
    /// The registered clients, each with its scope, its resources and its
    /// payload.
    pub clients: Vec<RegisteredClient<P>>,
}

impl<P> Default for OAuthServerConfig<P> {
    fn default() -> Self {
        Self {
            clients: Vec::new(),
        }
    }
}

impl<P: DeserializeOwned + Clone + Send + Sync + 'static> Config for OAuthServerConfig<P> {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            clients: env.json("CLIENTS")?.unwrap_or(base.clients),
        })
    }
}

impl<P> Validate for OAuthServerConfig<P> {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        if self.clients.is_empty() {
            errors.add("clients", ValidationError::new("at_least_one_client"));
        }
        let mut seen = HashSet::new();
        for client in &self.clients {
            let mut refuse = |field: &str, code: &'static str| {
                errors.errors_mut().insert(
                    format!("clients[{}].{field}", client.client_id).into(),
                    ValidationErrorsKind::Field(vec![ValidationError::new(code)]),
                );
            };
            if client.client_id.is_empty() {
                refuse("client_id", "required");
            } else if !seen.insert(client.client_id.as_str()) {
                refuse("client_id", "duplicate");
            }
            if client.client_secret.expose_secret().is_empty() {
                refuse("client_secret", "required");
            }
            if client.resources.is_empty() {
                refuse("resources", "at_least_one_resource");
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESOURCE: &str = "https://api.example.com";

    fn client(id: &str, secret: &str, resources: &[&str]) -> RegisteredClient<u32> {
        RegisteredClient::new(id, secret, ["posts:read"], resources.iter().copied(), 7)
    }

    fn refused(config: &OAuthServerConfig<u32>) -> Vec<String> {
        let mut keys: Vec<String> = config
            .validate()
            .expect_err("refused")
            .errors()
            .keys()
            .map(ToString::to_string)
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn a_registry_of_distinct_clients_each_with_a_secret_and_a_resource_passes() {
        let config = OAuthServerConfig {
            clients: vec![
                client("ci", "s3cret", &[RESOURCE]),
                client("ci-prod", "s3cret", &[RESOURCE]),
            ],
        };
        config.validate().expect("valid");
    }

    #[test]
    fn an_empty_registry_is_refused() {
        assert_eq!(refused(&OAuthServerConfig::default()), ["clients"]);
    }

    #[test]
    fn each_client_that_could_serve_no_token_or_the_wrong_one_is_refused_by_its_id() {
        let config = OAuthServerConfig {
            clients: vec![
                client("ci", "s3cret-never-shown", &[RESOURCE]),
                client("ci", "other-secret-never-shown", &[RESOURCE]),
                client("nowhere", "s3cret-never-shown", &[]),
                client("open", "", &[RESOURCE]),
                client("", "s3cret-never-shown", &[RESOURCE]),
            ],
        };

        assert_eq!(
            refused(&config),
            [
                "clients[].client_id",
                "clients[ci].client_id",
                "clients[nowhere].resources",
                "clients[open].client_secret",
            ]
        );
        let rendered = nest_rs_config::ConfigError::validation(
            <OAuthServerConfig<u32> as nest_rs_config::Namespaced>::NAMESPACE,
            config.validate().expect_err("refused"),
        )
        .to_string();
        assert!(!rendered.contains("never-shown"), "{rendered}");
    }

    #[test]
    fn the_namespace_is_the_crates_stem() {
        use nest_rs_config::Namespaced as _;
        assert_eq!(OAuthServerConfig::<u32>::NAMESPACE, "oauth__server");
    }

    #[test]
    fn the_clients_are_read_from_their_variable_and_never_quoted_when_they_do_not_parse() {
        let env = ConfigService::with_vars(
            "oauth__server",
            [(
                "CLIENTS",
                r#"[{"client_id":"web","client_secret":"s3cret","scopes":["posts:read"],"resources":["https://api.example.com"],"payload":7}]"#,
            )],
        );
        let config = OAuthServerConfig::<u32>::from_env(&env, OAuthServerConfig::default())
            .expect("the clients parse");
        assert_eq!(config.clients.len(), 1);
        assert_eq!(config.clients[0].client_id, "web");
        assert_eq!(config.clients[0].payload, 7);

        let env = ConfigService::with_vars(
            "oauth__server",
            [(
                "CLIENTS",
                r#"[{"client_id":"web","client_secret":"hunter2-SECRET","scopes":"hunter2-SECRET","resources":[],"payload":7}]"#,
            )],
        );
        let refusal = OAuthServerConfig::<u32>::from_env(&env, OAuthServerConfig::default())
            .err()
            .map(|e| e.to_string())
            .expect("mistyped clients are refused");
        assert!(refusal.contains(&env.var_name("CLIENTS")), "{refusal}");
        assert!(!refusal.contains("hunter2"), "{refusal}");
    }
}

use nest_rs::config::{Config, ConfigService, config};
use nest_rs::oauth::server::RegisteredClient;
use serde::Deserialize;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

use crate::Role;
use crate::authz::constants;

const DEFAULT_ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac3e);

#[derive(Debug, Clone, Deserialize)]
pub struct ClientPayload {
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

#[config(namespace = "oauth", validate = "manual")]
#[derive(Clone)]
pub struct OAuthConfig {
    pub clients: Vec<RegisteredClient<ClientPayload>>,
    pub default_org_id: Uuid,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            clients: Vec::new(),
            default_org_id: DEFAULT_ORG,
        }
    }
}

impl Validate for OAuthConfig {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        if self.clients.is_empty() {
            errors.add("clients", ValidationError::new("at_least_one_client"));
        }
        for client in &self.clients {
            if client
                .scopes
                .iter()
                .any(|scope| !constants::ALL.contains(&scope.as_str()))
            {
                errors.errors_mut().insert(
                    format!("clients[{}].scopes", client.client_id).into(),
                    ValidationErrorsKind::Field(vec![ValidationError::new("unknown_scope")]),
                );
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Config for OAuthConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs::config::Result<Self> {
        let clients = env.json("CLIENTS")?.unwrap_or(base.clients);
        let default_org_id = env.parse("DEFAULT_ORG_ID")?.unwrap_or(base.default_org_id);
        Ok(Self {
            clients,
            default_org_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: &str, scopes: &[&str]) -> RegisteredClient<ClientPayload> {
        RegisteredClient {
            client_id: id.into(),
            client_secret: "s3cr3t".into(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            payload: ClientPayload {
                org_id: Uuid::nil(),
                roles: vec![Role::User],
            },
        }
    }

    #[test]
    fn empty_clients_fails_validation() {
        let cfg = OAuthConfig {
            clients: vec![],
            default_org_id: Uuid::nil(),
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.field_errors().contains_key("clients"));
    }

    #[test]
    fn non_empty_clients_passes_validation() {
        let cfg = OAuthConfig {
            clients: vec![client("ci-runner", &constants::ALL)],
            default_org_id: Uuid::nil(),
        };
        cfg.validate().expect("valid");
    }

    #[test]
    fn a_registered_scope_outside_the_delegated_set_fails_validation_naming_the_client() {
        let cfg = OAuthConfig {
            clients: vec![
                client("ci-runner", &[constants::POSTS_READ]),
                client("legacy-runner", &[constants::POSTS_READ, "admin"]),
            ],
            default_org_id: Uuid::nil(),
        };

        let errors = cfg.validate().unwrap_err();

        let refused: Vec<String> = errors.errors().keys().map(ToString::to_string).collect();
        assert_eq!(refused, ["clients[legacy-runner].scopes"]);
        let rendered =
            nest_rs::config::ConfigError::validation("oauth", errors.clone()).to_string();
        assert!(
            rendered.contains("clients[legacy-runner].scopes: unknown_scope"),
            "{rendered}"
        );
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
    }

    #[test]
    fn clients_are_read_from_their_variable() {
        let env = ConfigService::with_vars(
            "oauth",
            [(
                "CLIENTS",
                r#"[{"client_id":"web","client_secret":"s3cr3t","scopes":["posts:read"],"payload":{"org_id":"00000000-0000-0000-0000-000000000000","roles":["user"]}}]"#,
            )],
        );
        let cfg = OAuthConfig::from_env(&env, OAuthConfig::defaults()).expect("the clients parse");
        assert_eq!(cfg.clients.len(), 1);
        assert_eq!(cfg.clients[0].client_id, "web");
        assert_eq!(cfg.clients[0].scopes, [constants::POSTS_READ]);
        assert_eq!(cfg.clients[0].payload.roles, [Role::User]);
        cfg.validate().expect("valid");
    }

    const MISTYPED_CLIENTS: &str = r#"[{"client_id":"demo","client_secret":"hunter2-SECRET","scopes":"hunter2-SECRET","payload":{"org_id":"018f0000-0000-7000-8000-000000000000","roles":["admin"]}}]"#;

    #[test]
    fn a_clients_file_that_does_not_parse_is_refused_under_its_own_variable_without_its_content() {
        let path = std::env::temp_dir().join(format!("oauth-clients-{}", Uuid::now_v7()));
        std::fs::write(&path, MISTYPED_CLIENTS).expect("the temp dir is writable");
        let env = ConfigService::with_vars(
            "oauth",
            [("CLIENTS_FILE", path.to_str().expect("a UTF-8 temp path"))],
        );
        let refusal = OAuthConfig::from_env(&env, OAuthConfig::default())
            .err()
            .map(|e| e.to_string());
        std::fs::remove_file(&path).expect("the temp file is removable");

        let refusal = refusal.expect("a mistyped clients file is refused");
        assert!(refusal.contains(&env.var_name("CLIENTS_FILE")), "{refusal}");
        assert!(refusal.contains("invalid type"), "{refusal}");
        assert!(!refusal.contains("hunter2"), "{refusal}");
    }

    #[test]
    fn inline_clients_that_do_not_parse_are_refused_without_their_content() {
        let env = ConfigService::with_vars("oauth", [("CLIENTS", MISTYPED_CLIENTS)]);
        let refusal = OAuthConfig::from_env(&env, OAuthConfig::default())
            .err()
            .map(|e| e.to_string())
            .expect("mistyped inline clients are refused");

        assert!(refusal.contains(&env.var_name("CLIENTS")), "{refusal}");
        assert!(
            !refusal.contains(&env.var_name("CLIENTS_FILE")),
            "{refusal}"
        );
        assert!(refusal.contains("invalid type"), "{refusal}");
        assert!(!refusal.contains("hunter2"), "{refusal}");
    }

    #[test]
    fn default_org_constant_does_not_drift() {
        assert_eq!(
            DEFAULT_ORG,
            Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac3e),
        );
    }
}

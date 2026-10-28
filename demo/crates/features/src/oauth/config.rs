use nest_rs::config::{Config, ConfigService, config};
use nest_rs::oauth::server::RegisteredClient;
use serde::Deserialize;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

use crate::authz::constants;
use crate::{Role, Subject};

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
    pub sign_in_resources: Vec<String>,
    pub default_org_id: Uuid,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            clients: Vec::new(),
            sign_in_resources: Vec::new(),
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
        if self.sign_in_resources.is_empty() {
            errors.add(
                "sign_in_resources",
                ValidationError::new("at_least_one_resource"),
            );
        }
        for client in &self.clients {
            if client.resources.is_empty() {
                errors.errors_mut().insert(
                    format!("clients[{}].resources", client.client_id).into(),
                    ValidationErrorsKind::Field(vec![ValidationError::new(
                        "at_least_one_resource",
                    )]),
                );
            }
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
            if Subject::from(client.client_id.clone()).user_id().is_some() {
                errors.errors_mut().insert(
                    format!("clients[{}].client_id", client.client_id).into(),
                    ValidationErrorsKind::Field(vec![ValidationError::new("reads_as_a_user")]),
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
        let sign_in_resources = env.list("SIGN_IN_RESOURCES", base.sign_in_resources)?;
        let default_org_id = env.parse("DEFAULT_ORG_ID")?.unwrap_or(base.default_org_id);
        Ok(Self {
            clients,
            sign_in_resources,
            default_org_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESOURCE: &str = "https://api.example.com";

    fn client(id: &str, scopes: &[&str]) -> RegisteredClient<ClientPayload> {
        RegisteredClient::new(
            id,
            "s3cr3t",
            scopes.iter().copied(),
            [RESOURCE],
            ClientPayload {
                org_id: Uuid::nil(),
                roles: vec![Role::User],
            },
        )
    }

    #[test]
    fn empty_clients_fails_validation() {
        let cfg = OAuthConfig {
            clients: vec![],
            sign_in_resources: vec![RESOURCE.into()],
            default_org_id: Uuid::nil(),
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.field_errors().contains_key("clients"));
    }

    #[test]
    fn non_empty_clients_passes_validation() {
        let cfg = OAuthConfig {
            clients: vec![client("ci-runner", &constants::ALL)],
            sign_in_resources: vec![RESOURCE.into()],
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
            sign_in_resources: vec![RESOURCE.into()],
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
    fn a_client_id_a_uuid_parses_fails_validation_naming_the_client() {
        let user_shaped = Uuid::now_v7().to_string();
        let cfg = OAuthConfig {
            clients: vec![
                client("ci-runner", &[constants::POSTS_READ]),
                client(&user_shaped, &[constants::POSTS_READ]),
            ],
            sign_in_resources: vec![RESOURCE.into()],
            default_org_id: Uuid::nil(),
        };

        let errors = cfg.validate().unwrap_err();

        let refused: Vec<String> = errors.errors().keys().map(ToString::to_string).collect();
        assert_eq!(refused, [format!("clients[{user_shaped}].client_id")]);
        let rendered = nest_rs::config::ConfigError::validation("oauth", errors).to_string();
        assert!(rendered.contains("reads_as_a_user"), "{rendered}");
    }

    #[test]
    fn clients_and_sign_in_resources_are_read_from_their_variables() {
        let env = ConfigService::with_vars(
            "oauth",
            [
                (
                    "CLIENTS",
                    r#"[{"client_id":"web","client_secret":"s3cr3t","scopes":["posts:read"],"resources":["https://api.example.com"],"payload":{"org_id":"00000000-0000-0000-0000-000000000000","roles":["user"]}}]"#,
                ),
                (
                    "SIGN_IN_RESOURCES",
                    "https://api.example.com,https://mcp.example.com",
                ),
            ],
        );
        let cfg = OAuthConfig::from_env(&env, OAuthConfig::defaults()).expect("the clients parse");
        assert_eq!(cfg.clients.len(), 1);
        assert_eq!(cfg.clients[0].client_id, "web");
        assert_eq!(cfg.clients[0].scopes, [constants::POSTS_READ]);
        assert_eq!(cfg.clients[0].resources, [RESOURCE]);
        assert_eq!(cfg.clients[0].payload.roles, [Role::User]);
        assert_eq!(cfg.sign_in_resources, [RESOURCE, "https://mcp.example.com"]);
        cfg.validate().expect("valid");
    }

    #[test]
    fn a_registry_naming_no_resource_fails_validation_naming_what() {
        let mut nowhere = client("ci-runner", &[constants::POSTS_READ]);
        nowhere.resources.clear();
        let cfg = OAuthConfig {
            clients: vec![nowhere],
            sign_in_resources: vec![],
            default_org_id: Uuid::nil(),
        };

        let errors = cfg.validate().unwrap_err();

        let mut refused: Vec<String> = errors.errors().keys().map(ToString::to_string).collect();
        refused.sort();
        assert_eq!(
            refused,
            ["clients[ci-runner].resources", "sign_in_resources"]
        );
    }

    const MISTYPED_CLIENTS: &str = r#"[{"client_id":"demo","client_secret":"hunter2-SECRET","scopes":"hunter2-SECRET","resources":[],"payload":{"org_id":"018f0000-0000-7000-8000-000000000000","roles":["admin"]}}]"#;

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

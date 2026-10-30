use nest_rs::config::config;
use nest_rs::oauth::server::OAuthServerConfig;
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

#[config(namespace = "sign_in", validate = "manual")]
#[derive(Clone)]
pub struct SignInConfig {
    pub resources: Vec<String>,
    pub default_org_id: Uuid,
}

impl Default for SignInConfig {
    fn default() -> Self {
        Self {
            resources: Vec::new(),
            default_org_id: DEFAULT_ORG,
        }
    }
}

impl Validate for SignInConfig {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        if self.resources.is_empty() {
            errors.add("resources", ValidationError::new("at_least_one_resource"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

pub(crate) fn registered_within_policy(
    registry: &OAuthServerConfig<ClientPayload>,
) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::new();
    for client in &registry.clients {
        let mut refuse = |field: &str, code: &'static str| {
            errors.errors_mut().insert(
                format!("clients[{}].{field}", client.client_id).into(),
                ValidationErrorsKind::Field(vec![ValidationError::new(code)]),
            );
        };
        if client
            .scopes
            .iter()
            .any(|scope| !constants::ALL.contains(&scope.as_str()))
        {
            refuse("scopes", "unknown_scope");
        }
        if Subject::from(client.client_id.clone()).user_id().is_some() {
            refuse("client_id", "reads_as_a_user");
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use nest_rs::config::{Config, ConfigError, ConfigService, Namespaced};
    use nest_rs::oauth::server::RegisteredClient;

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

    fn refused(clients: Vec<RegisteredClient<ClientPayload>>) -> (Vec<String>, String) {
        let errors = registered_within_policy(&OAuthServerConfig { clients }).expect_err("refused");
        let keys = errors.errors().keys().map(ToString::to_string).collect();
        let rendered = ConfigError::validation(
            <OAuthServerConfig<ClientPayload> as Namespaced>::NAMESPACE,
            errors,
        )
        .to_string();
        (keys, rendered)
    }

    #[test]
    fn clients_registered_for_the_delegated_scopes_pass() {
        registered_within_policy(&OAuthServerConfig {
            clients: vec![client("ci-runner", &constants::ALL)],
        })
        .expect("within the policy");
    }

    #[test]
    fn a_registered_scope_outside_the_delegated_set_is_refused_naming_the_client() {
        let (keys, rendered) = refused(vec![
            client("ci-runner", &[constants::POSTS_READ]),
            client("legacy-runner", &[constants::POSTS_READ, "admin"]),
        ]);

        assert_eq!(keys, ["clients[legacy-runner].scopes"]);
        assert!(
            rendered.contains("clients[legacy-runner].scopes: unknown_scope"),
            "{rendered}"
        );
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
    }

    #[test]
    fn a_client_id_a_uuid_parses_is_refused_naming_the_client() {
        let user_shaped = Uuid::now_v7().to_string();

        let (keys, rendered) = refused(vec![
            client("ci-runner", &[constants::POSTS_READ]),
            client(&user_shaped, &[constants::POSTS_READ]),
        ]);

        assert_eq!(keys, [format!("clients[{user_shaped}].client_id")]);
        assert!(rendered.contains("reads_as_a_user"), "{rendered}");
    }

    #[test]
    fn the_resources_and_the_default_org_are_read_from_their_variables() {
        let org = Uuid::now_v7();
        let env = ConfigService::with_vars(
            "sign_in",
            [
                (
                    "RESOURCES",
                    "https://api.example.com,https://mcp.example.com",
                ),
                ("DEFAULT_ORG_ID", &org.to_string()),
            ],
        );

        let cfg = SignInConfig::from_env(&env, SignInConfig::defaults()).expect("parses");

        assert_eq!(cfg.resources, [RESOURCE, "https://mcp.example.com"]);
        assert_eq!(cfg.default_org_id, org);
        cfg.validate().expect("valid");
    }

    #[test]
    fn a_sign_in_naming_no_resource_fails_validation() {
        let errors = SignInConfig::default().validate().unwrap_err();
        assert!(errors.field_errors().contains_key("resources"));
    }

    #[test]
    fn the_namespace_is_the_feature_s_own_word() {
        assert_eq!(<SignInConfig as Namespaced>::NAMESPACE, "sign_in");
    }

    #[test]
    fn default_org_constant_does_not_drift() {
        assert_eq!(
            DEFAULT_ORG,
            Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac3e),
        );
    }
}

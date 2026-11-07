use nest_rs::config::config;
use nest_rs::oauth::server::OAuthServerConfig;
use nest_rs::pipes::validator::{ValidationError, ValidationErrors, ValidationErrorsKind};
use serde::Deserialize;
use uuid::Uuid;

use crate::authz::constants;
use crate::{Role, Subject};

const DEFAULT_ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac3e);

#[derive(Debug, Clone, Deserialize)]
pub struct ClientPayload {
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

const FIRST_PARTY_CLIENT: &str = "publish-web";

#[config(namespace = "sign_in")]
#[derive(Clone)]
pub struct SignInConfig {
    #[validate(length(min = 1, code = "at_least_one_resource"))]
    pub resources: Vec<String>,
    pub default_org_id: Uuid,
    #[validate(length(min = 1, code = "required"))]
    pub client_id: String,
}

impl Default for SignInConfig {
    fn default() -> Self {
        Self {
            resources: Vec::new(),
            default_org_id: DEFAULT_ORG,
            client_id: FIRST_PARTY_CLIENT.into(),
        }
    }
}

pub(crate) fn registered_within_policy(
    registry: &OAuthServerConfig<ClientPayload>,
    sign_in: &SignInConfig,
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
            .any(|scope| !constants::ALL.iter().any(|known| scope == known))
        {
            refuse("scopes", "unknown_scope");
        }
        if Subject::from(client.client_id.clone()).user_id().is_some() {
            refuse("client_id", "reads_as_a_user");
        }
        if client.client_id == sign_in.client_id {
            refuse("client_id", "names_the_first_party_client");
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
    use nest_rs::config::{Config, ConfigError, ConfigService, Namespaced, var_name};
    use nest_rs::guards::Scope;
    use nest_rs::oauth::server::RegisteredClient;
    use nest_rs::pipes::validator::Validate;

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
        let errors =
            registered_within_policy(&OAuthServerConfig { clients }, &SignInConfig::default())
                .expect_err("refused");
        let keys = errors.errors().keys().map(ToString::to_string).collect();
        let rendered =
            ConfigError::validation(OAuthServerConfig::<ClientPayload>::NAMESPACE, errors)
                .to_string();
        (keys, rendered)
    }

    #[test]
    fn clients_registered_for_the_delegated_scopes_pass() {
        registered_within_policy(
            &OAuthServerConfig {
                clients: vec![client("ci-runner", &constants::ALL.map(Scope::as_str))],
            },
            &SignInConfig::default(),
        )
        .expect("within the policy");
    }

    #[test]
    fn a_client_registered_as_the_first_party_login_is_refused_naming_it() {
        let (keys, rendered) = refused(vec![
            client("ci-runner", &[constants::POSTS_READ.as_str()]),
            client(FIRST_PARTY_CLIENT, &[constants::POSTS_READ.as_str()]),
        ]);

        assert_eq!(keys, [format!("clients[{FIRST_PARTY_CLIENT}].client_id")]);
        assert!(
            rendered.contains("names_the_first_party_client"),
            "{rendered}"
        );
    }

    #[test]
    fn a_registered_scope_outside_the_delegated_set_is_refused_naming_the_client() {
        let (keys, rendered) = refused(vec![
            client("ci-runner", &[constants::POSTS_READ.as_str()]),
            client("legacy-runner", &[constants::POSTS_READ.as_str(), "admin"]),
        ]);

        assert_eq!(keys, ["clients[legacy-runner].scopes"]);
        assert!(
            rendered.contains(&format!(
                "clients[legacy-runner].scopes ({}): unknown_scope",
                var_name(OAuthServerConfig::<ClientPayload>::NAMESPACE, "CLIENTS")
            )),
            "{rendered}"
        );
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
    }

    #[test]
    fn a_client_id_a_uuid_parses_is_refused_naming_the_client() {
        let user_shaped = Uuid::now_v7().to_string();

        let (keys, rendered) = refused(vec![
            client("ci-runner", &[constants::POSTS_READ.as_str()]),
            client(&user_shaped, &[constants::POSTS_READ.as_str()]),
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
                ("CLIENT_ID", "publish-ios"),
            ],
        );

        let cfg = SignInConfig::from_env(&env, SignInConfig::default()).expect("parses");

        assert_eq!(cfg.resources, [RESOURCE, "https://mcp.example.com"]);
        assert_eq!(cfg.default_org_id, org);
        assert_eq!(cfg.client_id, "publish-ios");
        cfg.validate().expect("valid");
    }

    #[test]
    fn a_sign_in_naming_no_resource_fails_validation() {
        let errors = SignInConfig::default().validate().unwrap_err();
        let fields = errors.field_errors();
        assert_eq!(fields["resources"][0].code, "at_least_one_resource");
    }

    #[test]
    fn the_namespace_is_the_feature_s_own_word() {
        assert_eq!(SignInConfig::NAMESPACE, "sign_in");
    }

    #[test]
    fn default_org_constant_does_not_drift() {
        assert_eq!(
            DEFAULT_ORG,
            Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac3e),
        );
    }
}

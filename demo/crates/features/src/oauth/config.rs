use nest_rs::authn::RegisteredClient;
use nest_rs::config::{Config, ConfigError, ConfigService, config};
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

const DEFAULT_ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac3e);

// Cross-field rules (a client id must be unique across the list), so this
// one writes `impl Validate` by hand and opts out of the derive.
#[config(namespace = "issuer", validate = "manual")]
#[derive(Clone)]
pub struct IssuerConfig {
    pub clients: Vec<RegisteredClient<Uuid>>,
    pub default_org_id: Uuid,
}

impl Default for IssuerConfig {
    fn default() -> Self {
        Self {
            clients: Vec::new(),
            default_org_id: DEFAULT_ORG,
        }
    }
}

impl Validate for IssuerConfig {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        if self.clients.is_empty() {
            errors.add("clients", ValidationError::new("at_least_one_client"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Config for IssuerConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs::config::Result<Self> {
        let clients = match env.get("CLIENTS") {
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|e| ConfigError::parse(env.var_name("CLIENTS"), e.to_string()))?,
            None => base.clients,
        };
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

    fn client(id: &str) -> RegisteredClient<Uuid> {
        RegisteredClient {
            client_id: id.into(),
            client_secret: "s3cr3t".into(),
            scopes: vec!["user".into()],
            payload: Uuid::nil(),
        }
    }

    #[test]
    fn empty_clients_fails_validation() {
        let cfg = IssuerConfig {
            clients: vec![],
            default_org_id: Uuid::nil(),
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.field_errors().contains_key("clients"));
    }

    #[test]
    fn non_empty_clients_passes_validation() {
        let cfg = IssuerConfig {
            clients: vec![client("ci-runner")],
            default_org_id: Uuid::nil(),
        };
        cfg.validate().expect("valid");
    }

    #[test]
    fn default_org_constant_does_not_drift() {
        assert_eq!(
            DEFAULT_ORG,
            Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac3e),
        );
    }
}

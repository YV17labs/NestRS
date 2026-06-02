use nestrs_config::{config, Config, ConfigError, ConfigService};
use serde::Deserialize;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

const DEFAULT_ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac3e);

#[config(namespace = "issuer")]
#[derive(Clone, Default)]
pub struct IssuerConfig {
    pub clients: Vec<RegisteredClient>,
    pub default_org_id: Uuid,
}

#[derive(Clone, Deserialize)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_secret: String,
    pub org_id: Uuid,
    pub scopes: Vec<String>,
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
    fn from_env(env: &ConfigService) -> nestrs_config::Result<Self> {
        let clients = match env.get("CLIENTS") {
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|e| ConfigError::parse(env.var("CLIENTS"), e.to_string()))?,
            None => Vec::new(),
        };
        let default_org_id = env.parse("DEFAULT_ORG_ID")?.unwrap_or(DEFAULT_ORG);
        Ok(Self {
            clients,
            default_org_id,
        })
    }
}

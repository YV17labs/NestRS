//! [`AuthnModule`] — wires a configured [`JwtService`](super::JwtService) as global infrastructure.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::jwt::{JwtConfig, JwtService};

pub struct AuthnModule;

impl AuthnModule {
    pub fn for_root(config: impl Into<Option<JwtConfig>>) -> AuthnSetup {
        AuthnSetup {
            pinned: config.into(),
        }
    }
}

pub struct AuthnSetup {
    pinned: Option<JwtConfig>,
}

impl DynamicModule for AuthnSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<JwtService, _, _>(|container| async move {
            let config = container
                .get::<JwtConfig>()
                .expect("JwtConfig is resolved by ConfigModule::provide_feature");
            let options = (*config)
                .clone()
                .into_options()
                .map_err(anyhow::Error::new)?;
            JwtService::new(options).map_err(anyhow::Error::new)
        })
    }
}

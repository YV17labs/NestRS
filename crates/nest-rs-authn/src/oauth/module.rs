//! [`OAuth2Module`] — wires a configured [`OAuth2Client`](super::OAuth2Client) as global infrastructure.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::oauth::{OAuth2Client, OAuth2Config};

pub struct OAuth2Module;

impl OAuth2Module {
    pub fn for_root(config: impl Into<Option<OAuth2Config>>) -> OAuth2Setup {
        OAuth2Setup {
            pinned: config.into(),
        }
    }
}

pub struct OAuth2Setup {
    pinned: Option<OAuth2Config>,
}

impl DynamicModule for OAuth2Setup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<OAuth2Client, _, _>(|container| async move {
            let config = container
                .get::<OAuth2Config>()
                .expect("OAuth2Config is resolved by ConfigModule::provide_feature");
            OAuth2Client::new((*config).clone()).map_err(anyhow::Error::new)
        })
    }
}

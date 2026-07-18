//! [`OAuth2Module`] — wires a configured [`OAuth2Client`](super::OAuth2Client) as global infrastructure.
//!
//! For **social login** (mounting GitHub/Google or a custom provider behind an
//! open, discovered provider contract), reach for `nest-rs-social` instead —
//! its providers compose this `OAuth2Client` as their shared flow. This module
//! stays for wiring a single OAuth2 client as generic infrastructure (e.g.
//! non-login API access).

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::oauth::{OAuth2Client, OAuth2Config};

/// DI module that provides a single configured [`OAuth2Client`] as global
/// infrastructure. For social login, prefer `nest-rs-social`; this is for a lone
/// OAuth2 client (e.g. non-login API access). See the module docs.
pub struct OAuth2Module;

impl OAuth2Module {
    /// `None` ⇒ load [`OAuth2Config`] from `NESTRS_AUTHN__*`; `Some(cfg)` pins
    /// it in code. Either way the [`OAuth2Client`] factory is registered.
    pub fn for_root(config: impl Into<Option<OAuth2Config>>) -> OAuth2Setup {
        OAuth2Setup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`OAuth2Module::for_root`]: provides the config
/// (pinned or env-loaded), then queues the [`OAuth2Client`] factory.
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

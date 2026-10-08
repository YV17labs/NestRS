//! Activation seam for HTTP: [`HttpModule::for_root`] attaches the [`HttpTransport`] at boot.

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{
    Collecting, ContainerBuilder, DynamicModule, Registering, TransportContribution,
};

use crate::config::HttpConfig;
use crate::transport::HttpTransport;

/// The HTTP activation seam. Import [`HttpModule::for_root`] in an app module's
/// `imports` to attach the [`HttpTransport`] at boot.
pub struct HttpModule;

impl HttpModule {
    /// `None` ⇒ load from `<PREFIX>_HTTP__*`; `Some(cfg)` pins in code.
    pub fn for_root(config: impl Into<Option<HttpConfig>>) -> HttpSetup {
        HttpSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`HttpModule::for_root`].
pub struct HttpSetup {
    pinned: Option<HttpConfig>,
}

impl DynamicModule for HttpSetup {
    fn module() -> TypeId {
        TypeId::of::<HttpModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "HttpTransport",
            build: |c| {
                #[expect(
                    clippy::expect_used,
                    reason = "provide_feature queued the config's factory in this module's collect"
                )]
                let cfg = c
                    .get::<HttpConfig>()
                    .expect("HttpConfig is resolved by ConfigModule::provide_feature");
                Ok(Box::new(HttpTransport::from_config(&cfg)?))
            },
        })
    }
}

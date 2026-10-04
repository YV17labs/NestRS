//! Activation seam for HTTP. Import [`HttpModule::for_root(...)`](HttpModule::for_root) in an
//! `AppModule.imports` and the framework attaches the
//! [`HttpTransport`] at boot. Every option lives on
//! [`HttpConfig`] (host + port + optional TLS), populated either by the
//! `NESTRS_HTTP__*` env scheme or by the pinned struct.

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, TransportContribution};

use crate::config::HttpConfig;
use crate::transport::HttpTransport;

/// The HTTP activation seam. Import [`HttpModule::for_root`] in an app module's
/// `imports` to attach the [`HttpTransport`] at boot.
pub struct HttpModule;

impl HttpModule {
    /// `None` ⇒ load from `NESTRS_HTTP__*`; `Some(cfg)` pins in code.
    pub fn for_root(config: impl Into<Option<HttpConfig>>) -> HttpSetup {
        HttpSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`HttpModule::for_root`]. Registers the
/// [`HttpConfig`] (pinned or env-loaded) and contributes the HTTP transport.
pub struct HttpSetup {
    pinned: Option<HttpConfig>,
}

impl DynamicModule for HttpSetup {
    fn module() -> TypeId {
        TypeId::of::<HttpModule>()
    }

    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
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

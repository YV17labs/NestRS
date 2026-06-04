//! Activation seam for HTTP. Import [`HttpModule::for_root(...)`] in an
//! `AppModule.imports` and the framework attaches the
//! [`HttpTransport`](crate::HttpTransport) at boot. Every option lives on
//! [`HttpConfig`] (host + port + optional TLS), populated either by the
//! `NESTRS_HTTP__*` env scheme or by the pinned struct.

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule, TransportContribution};

use crate::config::HttpConfig;
use crate::transport::HttpTransport;

pub struct HttpModule;

impl HttpModule {
    /// `None` ⇒ load from `NESTRS_HTTP__*`; `Some(cfg)` pins in code.
    pub fn for_root(config: impl Into<Option<HttpConfig>>) -> HttpSetup {
        HttpSetup {
            pinned: config.into(),
        }
    }
}

pub struct HttpSetup {
    pinned: Option<HttpConfig>,
}

impl DynamicModule for HttpSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "HttpTransport",
            build: |c| {
                let cfg = c
                    .get::<HttpConfig>()
                    .expect("HttpConfig is resolved by ConfigModule::provide_feature");
                let mut http = HttpTransport::new().bind(format!("{}:{}", cfg.host, cfg.port));
                if let Some(tls) = cfg.tls.clone() {
                    http = http.tls(tls);
                }
                if let Some(cors) = cfg.cors.clone() {
                    http = http.cors(cors.into_middleware()?);
                }
                if cfg.server_header {
                    http = http.server_header(concat!("nestrs/", env!("CARGO_PKG_VERSION")));
                }
                Ok(Box::new(http))
            },
        })
    }
}

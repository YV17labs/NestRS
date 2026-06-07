//! Activation seam for HTTP. Import [`HttpModule::for_root(...)`] in an
//! `AppModule.imports` and the framework attaches the
//! [`HttpTransport`](crate::HttpTransport) at boot. Every option lives on
//! [`HttpConfig`] (host + port + optional TLS), populated either by the
//! `NESTRS_HTTP__*` env scheme or by the pinned struct.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, TransportContribution};

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
                if let Some(prefix) = cfg.global_prefix.clone() {
                    http = http.global_prefix(prefix);
                }
                if let Some(limit) = cfg.max_body_bytes {
                    // Install the per-request cap as a request-data entry
                    // — the `RawBody` extractor reads it back from the
                    // extensions. Using `EndpointExt::data` directly here
                    // (rather than a custom `Interceptor` impl) keeps this
                    // crate free of the cross-transport `Interceptor`
                    // trait that would otherwise close a dep cycle.
                    http = http.max_body_bytes(limit);
                }
                Ok(Box::new(http))
            },
        })
    }
}

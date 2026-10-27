//! Activation seam for HTTP: importing [`HttpModule`] attaches the
//! [`HttpTransport`] at boot, configured through [`HttpModule::for_root`].

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering};

use crate::config::HttpConfig;
use crate::transport::HttpTransport;

/// The HTTP activation seam. The app imports [`HttpModule::for_root`] to
/// attach the [`HttpTransport`] at boot; a feature that serves over HTTP
/// imports `HttpModule` bare beside it, and the app still runs one server.
pub struct HttpModule;

impl HttpModule {
    /// `None` loads [`HttpConfig`] from `<PREFIX>_HTTP__*`; `Some(cfg)` pins
    /// the base those variables overlay, field by field.
    pub fn for_root(config: impl Into<Option<HttpConfig>>) -> HttpSetup {
        ConfigModule::setup(config.into())
    }
}

/// The configured import [`HttpModule::for_root`] returns.
pub type HttpSetup = ConfigSetup<HttpModule, HttpConfig>;

impl Module for HttpModule {
    // A bare import still reads `<PREFIX>_HTTP__*`; a `for_root(Some(cfg))`'s
    // declared value supersedes this env-only one.
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(None::<HttpConfig>, builder)
    }

    // Registered once however many modules import it, so a feature importing
    // it beside the app's `for_root` binds no second server to the port.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|c| {
            let cfg = ConfigModule::resolved::<HttpConfig>(c)?;
            HttpTransport::from_config(&cfg)
        })
    }
}

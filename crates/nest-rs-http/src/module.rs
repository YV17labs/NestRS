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

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, ContestedDeclarationError, Discovery, TransportContribution, module};

    use super::*;

    /// The transports `M`'s app attaches, by name.
    async fn transports<M: Module>() -> Vec<String> {
        let app = App::builder()
            .module::<M>()
            .build()
            .await
            .unwrap_or_else(|refused| panic!("the app boots: {refused:#}"));
        Discovery::new(app.container())
            .meta::<TransportContribution>()
            .iter()
            .map(|contribution| contribution.meta.name().to_owned())
            .collect()
    }

    /// A feature serving over HTTP, which declares the dependency bare.
    #[module(imports = [HttpModule])]
    struct ServesOverHttp;

    #[module(imports = [HttpModule::for_root(None), ServesOverHttp])]
    struct ConfiguredThenFeature;

    #[module(imports = [ServesOverHttp, HttpModule::for_root(None)])]
    struct FeatureThenConfigured;

    #[module(imports = [HttpModule::for_root(None), HttpModule::for_root(None)])]
    struct TwiceUnpinned;

    #[tokio::test]
    async fn a_bare_import_beside_the_configured_one_attaches_one_transport() {
        assert_eq!(
            transports::<ConfiguredThenFeature>().await,
            ["HttpTransport"]
        );
        assert_eq!(
            transports::<FeatureThenConfigured>().await,
            ["HttpTransport"]
        );
    }

    #[tokio::test]
    async fn two_imports_attach_one_transport() {
        assert_eq!(transports::<TwiceUnpinned>().await, ["HttpTransport"]);
    }

    fn pinned() -> HttpSetup {
        HttpModule::for_root(HttpConfig {
            port: 3999,
            ..HttpConfig::default()
        })
    }

    #[module(imports = [pinned(), pinned()])]
    struct TwicePinned;

    #[tokio::test]
    async fn two_pinned_configs_fail_the_boot_naming_both() {
        let Err(refused) = App::builder().module::<TwicePinned>().build().await else {
            panic!("two pins of one config must not boot");
        };
        let contested = refused
            .downcast_ref::<ContestedDeclarationError>()
            .unwrap_or_else(|| panic!("not the contest: {refused:#}"));
        assert_eq!(contested.type_name, std::any::type_name::<HttpConfig>());
        assert_eq!(
            (contested.first.as_str(), contested.second.as_str()),
            (
                "`pinned(..)` at `imports[0]` of `TwicePinned`",
                "`pinned(..)` at `imports[1]` of `TwicePinned`",
            )
        );
    }
}

//! [`ResilienceModule`] — the port's seam: one [`ResiliencePolicy`] per
//! instance, built from its [`ResilienceConfig`].

use std::borrow::Borrow;
use std::marker::PhantomData;

use nest_rs_config::{ConfigModule, ConfigSetup, ForInstance};
use nest_rs_core::{Collecting, ContainerBuilder, Instance, Module, Registering, Unnamed};

use crate::config::ResilienceConfig;
use crate::policy::ResiliencePolicy;

/// The resilience port for the instance `I`: the [`ResiliencePolicy<I>`] the
/// module whose provider injects it imports, bare, which reads its variables.
/// It is imported by type, so every module importing one instance shares one
/// policy, and one breaker.
///
/// The default instance reads `<PREFIX>_RESILIENCE__*`; a `#[instance] struct
/// Upstream` reads `<PREFIX>_RESILIENCE__UPSTREAM__*` and is injected as
/// `Arc<ResiliencePolicy<Upstream>>`.
///
/// ```
/// use std::time::Duration;
///
/// use nest_rs_core::{instance, module};
/// use nest_rs_resilience::{ResilienceConfig, ResilienceModule};
///
/// #[instance]
/// pub struct Payments;
///
/// #[instance]
/// pub struct Carrier;
///
/// #[module(imports = [
///     ResilienceModule<Payments>,
///     ResilienceModule::for_root((Carrier, ResilienceConfig {
///         timeout: Duration::from_secs(2),
///         ..ResilienceConfig::default()
///     })),
/// ])]
/// pub struct AppModule;
/// ```
pub struct ResilienceModule<I = Unnamed>(PhantomData<fn() -> I>);

impl ResilienceModule {
    /// Pin the config of one instance, the base its variables overlay, per
    /// field: the default instance takes a [`ResilienceConfig`], a named one
    /// `(Marker, config)`. The bare import, `ResilienceModule` or
    /// `ResilienceModule<Marker>`, reads those variables over the defaults.
    pub fn for_root<I: Instance>(
        config: impl Into<ForInstance<I, ResilienceConfig>>,
    ) -> ResilienceSetup<I> {
        ConfigModule::setup(config)
    }
}

/// The configured import [`ResilienceModule::for_root`] returns for the
/// instance `I`.
pub type ResilienceSetup<I = Unnamed> = ConfigSetup<ResilienceModule<I>, ResilienceConfig, I>;

impl<I: Instance> Module for ResilienceModule<I> {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<I, ResilienceConfig>(builder)
            .provide_factory_after::<ResiliencePolicy<I>, (I::Of<ResilienceConfig>,), _, _>(
                |container| async move {
                    let held = ConfigModule::resolved_for::<I, ResilienceConfig>(&container)?;
                    let config: &ResilienceConfig = (*held).borrow();
                    Ok(ResiliencePolicy::<I>::new(config)?)
                },
            )
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

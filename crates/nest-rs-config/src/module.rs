//! [`ConfigModule`] — the `ConfigModule.for_root` / `for_feature` DI wiring.

use std::any::TypeId;

use std::marker::PhantomData;

use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Module, Registering};

use crate::config::Config;
use crate::environment::Environment;

/// Sole owner of config loading: [`for_feature`](Self::for_feature) declares a
/// config, [`setup`](Self::setup) backs a module's `for_root`, and
/// [`provide_feature`](Self::provide_feature) is the primitive both route
/// through.
///
/// Config reads see the `.env` cascade without any import; [`Environment::init`]
/// at the top of `main` also publishes it into `std::env` for non-config readers.
pub struct ConfigModule;

impl ConfigModule {
    /// Register the active [`Environment`] so a provider can inject
    /// `Arc<Environment>` and branch on the profile. Its position among
    /// `imports` carries no meaning.
    pub fn for_root() -> ConfigRootSetup {
        ConfigRootSetup
    }

    /// **Declare** that `C` exists and must be loaded — it does not configure
    /// it. Loads in the factory phase from the environment over `C::defaults()`,
    /// becoming global infrastructure; a test that seeds `C` wins over it. A
    /// base is pinned on the owning module's own `for_root(cfg)`.
    pub fn for_feature<C: Config>() -> ConfigFeatureSetup<C> {
        ConfigFeatureSetup(PhantomData)
    }

    /// Queue the factory that resolves `C` for the boot. `None` loads from the
    /// environment over `C::defaults()`; `Some(cfg)` makes `cfg` the **base**
    /// the environment overlays — what an app passes to
    /// `Module::for_root(config)`. Every configurable module's `for_root` routes
    /// through this.
    ///
    /// Both arms resolve through [`Config::resolve`], so the override is per
    /// field. A pinned base is a declaration
    /// ([`provide_declared_factory`](ContainerBuilder::provide_declared_factory)):
    /// it supersedes the environment-only factory a bare import of the same
    /// module queues, wherever the two fall in `imports = [..]`, and two pinned
    /// bases fail the boot
    /// ([`ContestedDeclarationError`](nest_rs_core::ContestedDeclarationError)).
    pub fn provide_feature<C: Config>(
        pinned: Option<C>,
        builder: ContainerBuilder,
    ) -> ContainerBuilder {
        match pinned {
            Some(base) => builder.provide_declared_factory::<C, _, _>(
                "A config has one seam — pin it once, on the `for_root` of the module that owns \
                 it, and let every other import of that module stay bare.",
                |_| async move { C::resolve(Some(base)).map_err(anyhow::Error::from) },
            ),
            None => builder.provide_factory::<C, _, _>(|_| async move {
                C::resolve(None).map_err(anyhow::Error::from)
            }),
        }
    }

    /// The whole body of a `for_root` whose module pins a config and does
    /// nothing else:
    ///
    /// ```
    /// # use nest_rs_config::{Config, ConfigModule, ConfigService, ConfigSetup, config};
    /// # use nest_rs_core::{App, module};
    /// # #[config(namespace = "ws")]
    /// # #[derive(Clone, Debug, Default)]
    /// # pub struct WsConfig {
    /// #     pub max_message_bytes: usize,
    /// # }
    /// # impl Config for WsConfig {
    /// #     fn from_env(_: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
    /// #         Ok(base)
    /// #     }
    /// # }
    /// # #[module(imports = [ConfigModule::for_feature::<WsConfig>()])]
    /// # pub struct WsModule;
    /// pub type WsSetup = ConfigSetup<WsModule, WsConfig>;
    ///
    /// impl WsModule {
    ///     pub fn for_root(config: impl Into<Option<WsConfig>>) -> WsSetup {
    ///         ConfigModule::setup(config)
    ///     }
    /// }
    /// # #[module(imports = [WsModule::for_root(WsConfig { max_message_bytes: 42 })])]
    /// # struct AppModule;
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// #     let app = App::builder().module::<AppModule>().build().await?;
    /// #     let pinned = app.container().get::<WsConfig>().map(|c| c.max_message_bytes);
    /// #     assert_eq!(pinned, Some(42));
    /// #     Ok(())
    /// # }
    /// ```
    pub fn setup<M: Module, C: Config>(pinned: impl Into<Option<C>>) -> ConfigSetup<M, C> {
        ConfigSetup {
            pinned: pinned.into(),
            module: PhantomData,
        }
    }
}

/// The [`DynamicModule`] behind a `for_root` that only pins a config: resolves
/// `C` (environment over the pinned base, per field) in the factory phase, and
/// imports `M`, its ordinary wiring, in both phases. Built by
/// [`ConfigModule::setup`].
pub struct ConfigSetup<M, C> {
    pinned: Option<C>,
    module: PhantomData<fn() -> M>,
}

impl<M: Module + 'static, C: Config> DynamicModule for ConfigSetup<M, C> {
    fn module() -> TypeId {
        TypeId::of::<M>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder.import::<M>())
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<M>()
    }
}

/// The import produced by [`ConfigModule::for_feature`]. Queues a factory that
/// loads and validates `C` in the factory phase, as global infrastructure.
pub struct ConfigFeatureSetup<C>(PhantomData<fn() -> C>);

impl<C: Config> DynamicModule for ConfigFeatureSetup<C> {
    fn module() -> TypeId {
        TypeId::of::<ConfigModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<C>(None, builder)
    }
}

/// The import produced by [`ConfigModule::for_root`]. Registers the active
/// [`Environment`] so later config loads see the resolved `.env` cascade.
pub struct ConfigRootSetup;

impl DynamicModule for ConfigRootSetup {
    fn module() -> TypeId {
        TypeId::of::<ConfigModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        // No `set_var` on the boot path: a spawned worker's `getenv` could race it.
        builder.provide(Environment::from_env())
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    /// The `for_root` / `for_feature` seam, exercised through a real boot.
    mod seam {
        use nest_rs_core::{App, Collecting, ContainerBuilder, Module, Registering, module};
        use validator::Validate;

        use super::*;
        use crate::ConfigService;

        /// A namespace no deployment sets, so these tests read only what they pin.
        #[derive(Clone, Default, Validate)]
        struct SeamConfig {
            bucket: String,
        }

        impl crate::Namespaced for SeamConfig {
            const NAMESPACE: &'static str = "config_seam_guard";
        }

        impl Config for SeamConfig {
            fn from_env(env: &ConfigService, base: Self) -> crate::Result<Self> {
                Ok(Self {
                    bucket: env.get("BUCKET")?.unwrap_or(base.bucket),
                })
            }
        }

        /// Stands in for a framework module that registers its own config unpinned.
        struct OwnerModule;

        impl Module for OwnerModule {
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                builder
            }
            fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
                ConfigModule::provide_feature::<SeamConfig>(None, builder)
            }
        }

        fn pin() -> ConfigSetup<OwnerModule, SeamConfig> {
            ConfigModule::setup(SeamConfig {
                bucket: "pinned".into(),
            })
        }

        #[module(imports = [OwnerModule, pin()])]
        struct BareImportFirst;

        #[module(imports = [pin(), OwnerModule])]
        struct PinFirst;

        #[module(imports = [pin(), pin()])]
        struct TwoPins;

        #[module(imports = [OwnerModule, pin()])]
        struct FirstPinner;

        #[module(imports = [pin()])]
        struct SecondPinner;

        #[module(imports = [FirstPinner, SecondPinner])]
        struct PinnedInTwoModules;

        /// Factories are first-queued-wins, yet a pin beats a bare import in
        /// either order.
        #[tokio::test]
        async fn a_pin_survives_a_bare_import_listed_before_it() {
            for (label, cfg) in [
                ("bare import first", boot::<BareImportFirst>().await),
                ("pin first", boot::<PinFirst>().await),
            ] {
                assert_eq!(
                    cfg.bucket, "pinned",
                    "{label}: import order decided the value"
                );
            }
        }

        #[tokio::test]
        async fn two_pinned_bases_for_one_config_fail_the_boot() {
            let err = match App::builder().module::<TwoPins>().build().await {
                Ok(_) => panic!("two pinned bases for one config must not boot"),
                Err(err) => err.to_string(),
            };
            assert!(err.contains("contested declaration"), "{err}");
            assert!(
                err.contains("SeamConfig"),
                "the failure names the config: {err}"
            );
            assert!(
                err.contains("pin it once"),
                "the config seam supplies its own remedy, not a generic one: {err}"
            );
            assert!(
                err.contains(
                    "by `pin(..)` at `imports[0]` of `TwoPins`, and by `pin(..)` at \
                     `imports[1]` of `TwoPins`"
                ),
                "both declarations are named, by position: {err}"
            );
        }

        #[tokio::test]
        async fn two_pins_in_two_modules_are_both_named() {
            let err = match App::builder().module::<PinnedInTwoModules>().build().await {
                Ok(_) => panic!("two pinned bases for one config must not boot"),
                Err(err) => err.to_string(),
            };
            assert!(
                err.contains("`pin(..)` at `imports[1]` of `FirstPinner`")
                    && err.contains("`pin(..)` at `imports[0]` of `SecondPinner`"),
                "{err}"
            );
        }

        /// `App::new` runs `register` but never `collect`, so a queued factory would
        /// never be built.
        #[test]
        fn the_synchronous_boot_refuses_a_config_it_could_never_resolve() {
            let err = match App::new::<PinFirst>() {
                Ok(_) => panic!("App::new must not boot a container whose config never resolves"),
                Err(err) => err.to_string(),
            };
            assert!(err.contains("SeamConfig"), "{err}");
            assert!(
                err.contains("App::builder"),
                "the failure names the boot path that works: {err}"
            );
        }

        /// What a pinned module's own imports open beside its config.
        struct Opened;

        struct OpensModule;

        impl Module for OpensModule {
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                builder
            }
            fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
                builder.provide_factory(|_| async { Ok(Opened) })
            }
        }

        #[module(imports = [ConfigModule::for_feature::<SeamConfig>(), OpensModule])]
        struct OpeningOwner;

        #[module(imports = [ConfigModule::setup::<OpeningOwner, SeamConfig>(SeamConfig {
            bucket: "pinned".into(),
        })])]
        struct OnlyThePin;

        /// A setup is its module, configured: what that module's imports queue
        /// runs in the factory phase, as it does when the module is imported bare.
        #[tokio::test]
        async fn a_pinned_module_s_imports_queue_their_factories_in_collect() {
            let app = App::builder()
                .module::<OnlyThePin>()
                .build()
                .await
                .expect("the pinned module boots");
            assert!(app.container().get::<Opened>().is_some());
            assert_eq!(
                app.container()
                    .get::<SeamConfig>()
                    .map(|c| c.bucket.clone()),
                Some("pinned".to_owned())
            );
        }

        async fn boot<M: Module + 'static>() -> std::sync::Arc<SeamConfig> {
            App::builder()
                .module::<M>()
                .build()
                .await
                .expect("the module boots")
                .container()
                .get::<SeamConfig>()
                .expect("the config resolves")
        }
    }

    use super::*;
    use crate::service::var_name;
    use nest_rs_core::{App, module};

    #[module(imports = [ConfigModule::for_root()])]
    struct RootOnly;

    /// `ConfigModule::for_root()` registers the active [`Environment`] and does not
    /// merge the `.env` cascade into the process environment.
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test asserts the process environment was left alone"
    )]
    fn for_root_collect_does_not_publish_the_cascade_into_the_process_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                &format!(
                    "{}=from_dotenv",
                    var_name("for_root_guard", "SHOULD_STAY_UNSET"),
                ),
            )?;

            let app = App::new::<RootOnly>().expect("an app importing the root boots");
            let container = app.container();

            assert!(
                container.get::<Environment>().is_some(),
                "collect registers the active Environment",
            );
            assert!(
                std::env::var(var_name("for_root_guard", "SHOULD_STAY_UNSET")).is_err(),
                "importing ConfigModule::for_root() must not write the cascade into std::env",
            );
            Ok(())
        });
    }

    /// The read path, through the seam a config actually uses: resolving a value
    /// sees the dotenv file without publishing it.
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test asserts the process environment was left alone"
    )]
    fn a_config_read_sees_the_cascade_without_publishing_it() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                &format!("{}=from_dotenv", var_name("readpath_guard", "URL")),
            )?;

            let value = crate::ConfigService::for_namespace("readpath_guard")
                .get("URL")
                .unwrap();
            assert_eq!(
                value.as_deref(),
                Some("from_dotenv"),
                "the read path sees the cascade",
            );
            assert!(
                std::env::var(var_name("readpath_guard", "URL")).is_err(),
                "a config read resolves through the in-crate map, never through std::env",
            );
            Ok(())
        });
    }
}

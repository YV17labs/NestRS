//! [`ConfigModule`] — the `ConfigModule.for_root` / `for_feature` DI wiring.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_core::{
    Collecting, Container, ContainerBuilder, DynamicModule, Instance, Module, Registering, Unnamed,
};

use crate::config::{Config, check_seed, resolve_instance};
use crate::environment::Environment;
use crate::error::ConfigNotResolvedError;
use crate::for_instance::ForInstance;

/// Sole owner of config loading: [`for_feature`](Self::for_feature) declares a
/// config, [`setup`](Self::setup) backs a module's `for_root`, and
/// [`provide_feature`](Self::provide_feature) is the primitive both route
/// through.
///
/// Config reads see the `.env` cascade without any import; `#[nest_rs::main]`
/// also publishes it into `std::env`, before the runtime exists, for readers
/// that know only the process environment.
pub struct ConfigModule;

impl ConfigModule {
    /// Register the active [`Environment`] so a provider can inject
    /// `Arc<Environment>` and branch on the profile. Its position among
    /// `imports` carries no meaning. A process that declares no
    /// `<PREFIX>_ENV` runs as production, and the boot says so once at `info`.
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

    /// Queue the factory that resolves `C` for the boot, for one instance. `None`
    /// loads from the environment over `C::defaults()`; `Some(cfg)` makes `cfg`
    /// the **base** the environment overlays — what an app passes to
    /// `Module::for_root(config)`. Every configurable module's `for_root` routes
    /// through this.
    ///
    /// The default instance takes `x` and is held as `C`; a named one takes
    /// `(Marker, x)`, reads `C` under the namespace its name derives
    /// ([`instance_namespace`](crate::instance_namespace)) and is held as
    /// `Instanced<Marker, C>` ([`ForInstance`]).
    ///
    /// The default instance resolves through [`Config::resolve`], a named one
    /// takes its steps under its own namespace, so the override is per field. A pinned base is a declaration
    /// ([`provide_declared_factory`](ContainerBuilder::provide_declared_factory)):
    /// it supersedes the environment-only factory a bare import of the same
    /// module queues, wherever the two fall in `imports = [..]`, and two pinned
    /// bases for one instance fail the boot
    /// ([`ContestedDeclarationError`](nest_rs_core::ContestedDeclarationError)).
    pub fn provide_feature<I: Instance, C: Config>(
        config: impl Into<ForInstance<I, C>>,
        builder: ContainerBuilder,
    ) -> ContainerBuilder {
        // A seed skips the factory below, never the checks its read runs.
        let builder = nest_rs_core::__private::check_seed(
            builder,
            TypeId::of::<I::Of<C>>(),
            check_seed::<C, I>,
        );
        match config.into().into_pinned() {
            Some(base) => builder.provide_declared_factory::<I::Of<C>, _, _>(
                "A config has one seam — pin it once, on the `for_root` of the module that owns \
                 it, and let every other import of that module stay bare.",
                |_| async move {
                    resolve_instance::<C, I>(Some(base))
                        .map_err(anyhow::Error::from)
                },
            ),
            None => builder.provide_factory::<I::Of<C>, _, _>(|_| async move {
                resolve_instance::<C, I>(None)
                    .map_err(anyhow::Error::from)
            }),
        }
    }

    /// The `C` this boot resolved, or the refusal naming it: what a module
    /// reads the config its `collect` queued with — a `register` files the
    /// refusal with [`ContainerBuilder::refuse`], a factory naming `C` among
    /// its inputs returns it.
    ///
    /// ```
    /// # use std::any::TypeId;
    /// # use nest_rs_config::{Config, ConfigModule, ConfigService, config};
    /// # use nest_rs_core::{App, Collecting, ContainerBuilder, DynamicModule, module};
    /// # #[config(namespace = "bucket_guard")]
    /// # #[derive(Clone, Debug, Default)]
    /// # pub struct BucketConfig {
    /// #     pub name: String,
    /// # }
    /// # impl Config for BucketConfig {
    /// #     fn from_env(_: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
    /// #         Ok(base)
    /// #     }
    /// # }
    /// pub struct Bucket(String);
    ///
    /// pub struct BucketSetup(Option<BucketConfig>);
    ///
    /// impl DynamicModule for BucketSetup {
    ///     fn module() -> TypeId {
    ///         TypeId::of::<Self>()
    ///     }
    ///
    ///     fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
    ///         ConfigModule::provide_feature(self.0.clone(), builder)
    ///             .provide_factory_after::<Bucket, (BucketConfig,), _, _>(|container| async move {
    ///                 let config = ConfigModule::resolved::<BucketConfig>(&container)?;
    ///                 Ok(Bucket(config.name.clone()))
    ///             })
    ///     }
    /// }
    /// # #[module(imports = [BucketSetup(Some(BucketConfig { name: "media".into() }))])]
    /// # struct AppModule;
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// #     let app = App::builder().module::<AppModule>().build().await?;
    /// #     assert_eq!(app.container().get::<Bucket>().map(|b| b.0.clone()).as_deref(), Some("media"));
    /// #     Ok(())
    /// # }
    /// ```
    pub fn resolved<C: Config>(
        container: &Container,
    ) -> std::result::Result<Arc<C>, ConfigNotResolvedError> {
        container.get::<C>().ok_or(ConfigNotResolvedError {
            type_name: std::any::type_name::<C>(),
        })
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
    ///         ConfigModule::setup(config.into())
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
    ///
    /// An instance-capable module's `for_root` takes `impl Into<ForInstance<I,
    /// C>>` and hands it over as it is ([`ForInstance`] shows one).
    pub fn setup<M: Module, C: Config, I: Instance>(
        config: impl Into<ForInstance<I, C>>,
    ) -> ConfigSetup<M, C, I> {
        ConfigSetup {
            config: config.into(),
            module: PhantomData,
        }
    }
}

/// The [`DynamicModule`] behind a `for_root` that only pins a config: resolves
/// `C` for the instance `I` (environment over the pinned base, per field) in
/// the factory phase, and imports `M`, its ordinary wiring, in both phases.
/// Built by [`ConfigModule::setup`].
pub struct ConfigSetup<M, C, I = Unnamed> {
    config: ForInstance<I, C>,
    module: PhantomData<fn() -> M>,
}

impl<M: Module + 'static, C: Config, I: Instance> DynamicModule for ConfigSetup<M, C, I> {
    fn module() -> TypeId {
        TypeId::of::<M>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(self.config.clone(), builder.import::<M>())
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
        ConfigModule::provide_feature(None::<C>, builder)
    }
}

/// The import produced by [`ConfigModule::for_root`]. Registers the active
/// [`Environment`] so later config loads see the resolved `.env` cascade, and
/// files one `info` when the process declared none.
pub struct ConfigRootSetup;

impl DynamicModule for ConfigRootSetup {
    fn module() -> TypeId {
        TypeId::of::<ConfigModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let environment = Environment::from_env();
        if Environment::declared().is_none() {
            tracing::info!(
                target: crate::TARGET,
                environment = environment.as_str(),
                declared = false,
                "no environment declared; running as production",
            );
        }
        // No `set_var` on the boot path: a spawned worker's `getenv` could race it.
        builder.provide(environment)
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
                ConfigModule::provide_feature(None::<SeamConfig>, builder)
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

        #[module(imports = [ConfigModule::setup::<OpeningOwner, SeamConfig, Unnamed>(SeamConfig {
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

        #[test]
        fn a_config_no_factory_resolved_is_refused_naming_it() {
            let empty = Container::builder().build();
            let Err(refused) = ConfigModule::resolved::<SeamConfig>(&empty) else {
                panic!("an empty container resolved no config");
            };
            assert_eq!(refused.type_name, std::any::type_name::<SeamConfig>());
            assert_eq!(
                refused.to_string(),
                format!(
                    "`{}` was read before any factory resolved it: the module owning it is not \
                     imported, or is imported in a `register` alone, or the factory reading it \
                     does not name it among its inputs",
                    std::any::type_name::<SeamConfig>()
                )
            );
        }

        #[test]
        fn a_resolved_config_is_the_one_the_container_holds() {
            let container = Container::builder()
                .provide(SeamConfig {
                    bucket: "held".into(),
                })
                .build();
            let resolved = ConfigModule::resolved::<SeamConfig>(&container)
                .unwrap_or_else(|refused| panic!("{refused}"));
            assert_eq!(resolved.bucket, "held");
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

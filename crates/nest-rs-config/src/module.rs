//! [`ConfigModule`] — the seam every config's loading routes through: the
//! environment a bare import reads, and the base a `for_root` pins.

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

/// Sole owner of config loading: [`provide_feature`](Self::provide_feature)
/// queues what a module's bare import reads, [`provide_root`](Self::provide_root)
/// what its `for_root` pins, and [`setup`](Self::setup) is the whole body of a
/// `for_root` that pins and imports its module — as is the boot's own load of
/// a config a reachable provider injects: `#[inject] config: Arc<C>` is the
/// whole declaration of a config an app owns.
///
/// Config reads see the `.env` cascade without any import; `#[nest_rs::main]`
/// also publishes it into `std::env`, before the runtime exists, for readers
/// that know only the process environment. The first config a boot loads
/// registers the active [`Environment`], so a provider can inject
/// `Arc<Environment>` with no import of its own; a process declaring no
/// `<PREFIX>_ENV` runs as production, and that boot says so once at `info`.
pub struct ConfigModule;

impl ConfigModule {
    /// Queue the factory that resolves `C` for the instance `I` from the
    /// environment over `C::default()` — what a bare import of the module
    /// owning `C` queues. A pinned base for the same instance
    /// ([`provide_root`](Self::provide_root)) supersedes it, wherever the two
    /// fall in `imports = [..]`.
    ///
    /// The default instance is held as `C`; a named one reads `C` under the
    /// namespace its name derives ([`instance_namespace`](crate::instance_namespace))
    /// and is held as `Instanced<Marker, C>`. The default instance resolves
    /// through [`Config::resolve`], a named one takes its steps under its own
    /// namespace.
    pub fn provide_feature<I: Instance, C: Config>(builder: ContainerBuilder) -> ContainerBuilder {
        let builder = prepare::<I, C>(builder).provide_factory::<I::Of<C>, _, _>(|_| async move {
            resolve_instance::<C, I>(None, None).map_err(anyhow::Error::from)
        });
        nest_rs_core::__private::reading::<I::Of<C>>(builder)
    }

    /// Queue `config`, the base a module's `for_root` pinned for one instance,
    /// which the environment overlays per field: the default instance takes
    /// `x`, a named one `(Marker, x)` ([`ForInstance`]). A pinned base is a
    /// declaration
    /// ([`provide_declared_factory`](ContainerBuilder::provide_declared_factory)):
    /// it supersedes the environment-only factory a bare import of the same
    /// module queues, wherever the two fall in `imports = [..]`, and two
    /// pinned bases for one instance fail the boot
    /// ([`ContestedDeclarationError`](nest_rs_core::ContestedDeclarationError)).
    pub fn provide_root<I: Instance, C: Config>(
        config: impl Into<ForInstance<I, C>>,
        builder: ContainerBuilder,
    ) -> ContainerBuilder {
        let builder = prepare::<I, C>(builder);
        let base = config.into().into_pinned();
        let site = nest_rs_core::__private::declaring_site(&builder);
        let builder = builder.provide_declared_factory::<I::Of<C>, _, _>(
            "A config has one seam — pin it once, on the `for_root` of the module that owns it, \
             and let every other import of that module stay bare.",
            |_| async move {
                resolve_instance::<C, I>(Some(base), Some(site)).map_err(anyhow::Error::from)
            },
        );
        nest_rs_core::__private::reading::<I::Of<C>>(builder)
    }

    /// The `C` this boot resolved, or the refusal naming it: what a module
    /// reads the config its `collect` queued with — a `register` files the
    /// refusal with [`ContainerBuilder::refuse`], a factory naming `C` among
    /// its inputs returns it.
    ///
    /// ```
    /// # use std::any::TypeId;
    /// # use nest_rs_config::{ConfigModule, config};
    /// # use nest_rs_core::{App, Collecting, ContainerBuilder, DynamicModule, module};
    /// # #[config(namespace = "bucket_guard")]
    /// # #[derive(Clone, Debug, Default)]
    /// # pub struct BucketConfig {
    /// #     pub name: String,
    /// # }
    /// pub struct Bucket(String);
    ///
    /// pub struct BucketSetup(BucketConfig);
    ///
    /// impl DynamicModule for BucketSetup {
    ///     fn module() -> TypeId {
    ///         TypeId::of::<Self>()
    ///     }
    ///
    ///     fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
    ///         ConfigModule::provide_root(self.0.clone(), builder)
    ///             .provide_factory_after::<Bucket, (BucketConfig,), _, _>(|container| async move {
    ///                 let config = ConfigModule::resolved::<BucketConfig>(&container)?;
    ///                 Ok(Bucket(config.name.clone()))
    ///             })
    ///     }
    /// }
    /// # #[module(imports = [BucketSetup(BucketConfig { name: "media".into() })])]
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
        Self::resolved_for::<Unnamed, C>(container)
    }

    /// [`resolved`](Self::resolved) for the instance `I`: the `C` this boot
    /// resolved for it, held as `I::Of<C>` — the config itself for the default
    /// instance, `Instanced<I, C>` for a named one, either borrowed as `C`. A
    /// binding generic over the instance it binds names `I::Of<C>` among its
    /// factory's inputs and reads it here.
    ///
    /// ```
    /// # use std::borrow::Borrow;
    /// # use std::marker::PhantomData;
    /// # use nest_rs_config::{ConfigModule, config};
    /// # use nest_rs_core::{App, Collecting, ContainerBuilder, Instance, Module, Registering, instance, module};
    /// # #[config(namespace = "bucket_for")]
    /// # #[derive(Clone, Debug, Default)]
    /// # pub struct BucketConfig {
    /// #     pub name: String,
    /// # }
    /// pub struct Bucket<I>(String, PhantomData<fn() -> I>);
    ///
    /// pub struct BucketModule<I>(PhantomData<fn() -> I>);
    ///
    /// impl<I: Instance> Module for BucketModule<I> {
    ///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
    ///         builder
    ///     }
    ///
    ///     fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
    ///         ConfigModule::provide_feature::<I, BucketConfig>(builder)
    ///             .provide_factory_after::<Bucket<I>, (I::Of<BucketConfig>,), _, _>(
    ///                 |container| async move {
    ///                     let held = ConfigModule::resolved_for::<I, BucketConfig>(&container)?;
    ///                     let config: &BucketConfig = (*held).borrow();
    ///                     Ok(Bucket(config.name.clone(), PhantomData))
    ///                 },
    ///             )
    ///     }
    /// }
    ///
    /// #[instance]
    /// pub struct Uploads;
    /// # #[module(imports = [BucketModule::<Uploads>])]
    /// # struct AppModule;
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// #     let app = App::builder().module::<AppModule>().build().await?;
    /// #     assert!(app.container().get::<Bucket<Uploads>>().is_some());
    /// #     Ok(())
    /// # }
    /// ```
    pub fn resolved_for<I: Instance, C: Config>(
        container: &Container,
    ) -> std::result::Result<Arc<I::Of<C>>, ConfigNotResolvedError> {
        container.get::<I::Of<C>>().ok_or(ConfigNotResolvedError {
            type_name: std::any::type_name::<C>(),
            instance: I::NAME,
        })
    }

    /// The whole body of a `for_root` whose module pins a config and does
    /// nothing else: the bare import of the module reads its environment, and
    /// `for_root` takes only the value it pins.
    ///
    /// ```
    /// # use nest_rs_config::{ConfigModule, ConfigSetup, config};
    /// # use nest_rs_core::{App, module};
    /// # #[config(namespace = "ws")]
    /// # #[derive(Clone, Debug, Default)]
    /// # pub struct WsConfig {
    /// #     pub max_message_bytes: usize,
    /// # }
    /// # #[module(imports = [nest_rs_config::__private::feature::<WsConfig>()])]
    /// # pub struct WsModule;
    /// pub type WsSetup = ConfigSetup<WsModule, WsConfig>;
    ///
    /// impl WsModule {
    ///     pub fn for_root(config: WsConfig) -> WsSetup {
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

/// What both primitives do before they queue `C`'s factory for the instance
/// `I`: claim a named instance's namespace before any read, register the
/// active environment once, and hold a seed of `C` to the checks its read runs.
fn prepare<I: Instance, C: Config>(builder: ContainerBuilder) -> ContainerBuilder {
    if !I::NAME.is_empty() {
        crate::unclaimed::claim(&crate::instance_namespace::<I>(C::NAMESPACE));
    }
    nest_rs_core::__private::check_seed(
        environment(builder),
        TypeId::of::<I::Of<C>>(),
        check_seed::<C, I>,
    )
}

/// Register the active [`Environment`] the first time a boot loads a config,
/// filing then the one `info` that says a process declared none — unless a
/// test's override or a seed already holds it, the override then claimed.
fn environment(builder: ContainerBuilder) -> ContainerBuilder {
    let key = TypeId::of::<Environment>();
    if builder.contains(key) {
        return nest_rs_core::__private::stand_down(builder, key);
    }
    let (environment, declared) = Environment::read_declared();
    if declared.is_none() {
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
        ConfigModule::provide_root(self.config.clone(), builder.import::<M>())
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<M>()
    }
}

/// The import that queues `C`'s environment-over-defaults factory, for a
/// framework module that reads its config in `register` without injecting it.
/// Not API: an app's config loads once a provider injects it.
#[doc(hidden)]
pub struct ConfigFeatureSetup<C>(PhantomData<fn() -> C>);

/// [`ConfigFeatureSetup`] for `C`. Not API.
#[doc(hidden)]
pub fn feature<C: Config>() -> ConfigFeatureSetup<C> {
    ConfigFeatureSetup(PhantomData)
}

/// Queue `C`'s environment-over-defaults factory: what `#[config]` files for
/// the boot to call once a reachable provider injects `C`. Not API.
#[doc(hidden)]
pub fn queue_feature<C: Config>(builder: ContainerBuilder) -> ContainerBuilder {
    ConfigModule::provide_feature::<Unnamed, C>(builder)
}

/// The checks a seeded `C` passes before anything reads it: what `#[config]`
/// files beside [`queue_feature`]. Not API.
#[doc(hidden)]
pub fn check_feature<C: Config>(container: &Container) -> anyhow::Result<()> {
    check_seed::<C, Unnamed>(container)
}

impl<C: Config> DynamicModule for ConfigFeatureSetup<C> {
    fn module() -> TypeId {
        TypeId::of::<ConfigModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        queue_feature::<C>(builder)
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    /// The `for_root` seam, exercised through a real boot.
    mod seam {
        use nest_rs_core::{App, Collecting, ContainerBuilder, Module, Registering, module};
        use validator::Validate;

        use super::*;
        use crate::ConfigService;

        /// A namespace no deployment sets, so these tests read only what they pin.
        #[derive(Clone, Default, Validate)]
        pub(super) struct SeamConfig {
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
        pub(super) struct OwnerModule;

        impl Module for OwnerModule {
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                builder
            }
            fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
                ConfigModule::provide_feature::<Unnamed, SeamConfig>(builder)
            }
        }

        /// A `SeamConfig` a hand-built container holds before any module loads it.
        pub(super) fn seeded() -> SeamConfig {
            SeamConfig {
                bucket: "seeded".into(),
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

        #[module(imports = [feature::<SeamConfig>(), OpensModule])]
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
            let empty = Container::builder().build().expect("the container builds");
            let Err(refused) = ConfigModule::resolved::<SeamConfig>(&empty) else {
                panic!("an empty container resolved no config");
            };
            assert_eq!(refused.type_name, std::any::type_name::<SeamConfig>());
            assert_eq!(refused.instance, "");
            assert_eq!(
                refused.to_string(),
                "`SeamConfig` was read before any factory resolved it: the module owning it is not \
                 imported, or is imported in a `register` alone, or the factory reading it does \
                 not name it among its inputs"
            );
        }

        #[nest_rs_core::instance]
        struct Archive;

        #[test]
        fn a_named_instance_s_config_no_factory_resolved_is_refused_naming_the_instance() {
            let empty = Container::builder().build().expect("the container builds");
            let Err(refused) = ConfigModule::resolved_for::<Archive, SeamConfig>(&empty) else {
                panic!("an empty container resolved no config");
            };
            assert_eq!(refused.instance, "archive");
            assert!(
                refused.to_string().starts_with(
                    "`SeamConfig` of the instance `archive` was read before any factory resolved \
                     it"
                ),
                "{refused}"
            );
        }

        #[test]
        fn a_named_instance_s_config_is_read_where_the_container_holds_it() {
            let container = Container::builder()
                .provide(nest_rs_core::Instanced::<Archive, SeamConfig>::from(
                    SeamConfig {
                        bucket: "archived".into(),
                    },
                ))
                .build()
                .expect("the container builds");
            let held = ConfigModule::resolved_for::<Archive, SeamConfig>(&container)
                .unwrap_or_else(|refused| panic!("{refused}"));
            assert_eq!(held.bucket, "archived");
            assert!(
                ConfigModule::resolved::<SeamConfig>(&container).is_err(),
                "the default instance is another"
            );
        }

        #[test]
        fn a_resolved_config_is_the_one_the_container_holds() {
            let container = Container::builder()
                .provide(SeamConfig {
                    bucket: "held".into(),
                })
                .build()
                .expect("the container builds");
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

    /// The first config a boot loads registers the active [`Environment`]
    /// and does not merge the `.env` cascade into the process environment.
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test asserts the process environment was left alone"
    )]
    fn a_config_load_does_not_publish_the_cascade_into_the_process_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                &format!(
                    "{}=from_dotenv",
                    var_name("environment_guard", "SHOULD_STAY_UNSET"),
                ),
            )?;

            let container = Container::builder()
                .provide(seam::seeded())
                .import::<seam::OwnerModule>()
                .build()
                .expect("a module loading a seeded config builds");

            assert!(
                container.get::<Environment>().is_some(),
                "the config load registers the active Environment",
            );
            assert!(
                std::env::var(var_name("environment_guard", "SHOULD_STAY_UNSET")).is_err(),
                "a config load must not write the cascade into std::env",
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

//! [`ConfigModule`] — the `ConfigModule.for_root` / `for_feature` DI wiring.

use std::marker::PhantomData;

use nest_rs_core::{ContainerBuilder, DynamicModule, Module};

use crate::config::Config;
use crate::environment::Environment;

/// Sole owner of config loading: [`for_feature`](Self::for_feature) declares a
/// config, [`setup`](Self::setup) backs a module's `for_root`, and
/// [`provide_feature`](Self::provide_feature) is the primitive both route
/// through.
///
/// **What makes the `.env` cascade visible is [`Environment::init`] at the top
/// of `main`, not an import.** Config reads consult a lazily-built, in-crate map
/// (real env vars always win, and the process env is never mutated by a read),
/// so they see dotenv values whether or not anything registered
/// [`Environment`]. `init` additionally publishes the cascade into `std::env`,
/// which is what lets *non-config* consumers — a log filter, a spawned
/// `migrate` binary — see it too.
pub struct ConfigModule;

impl ConfigModule {
    /// Register the active [`Environment`] so a provider can inject
    /// `Arc<Environment>` and branch on the profile.
    ///
    /// It is **not** what makes the `.env` cascade readable — see the type
    /// docs — so its position among `imports` carries no meaning: every
    /// module's `collect` runs before any config factory does. Listing it first
    /// is a readability convention.
    pub fn for_root() -> ConfigRootSetup {
        ConfigRootSetup
    }

    /// **Declare** that `C` exists and must be loaded — it does not configure
    /// it. Loads in the factory phase from the environment over `C::defaults()`,
    /// becoming global infrastructure; a test that seeds `C` wins over it.
    ///
    /// Taking no value is the point, not an omission: `for_root` configures,
    /// `for_feature` registers, and a config reached through two seams is a
    /// config whose value depends on import order. The module that lists this
    /// import is the one that owns `C`, so its own `for_root(cfg)` is where a
    /// base is pinned.
    pub fn for_feature<C: Config>() -> ConfigFeatureSetup<C> {
        ConfigFeatureSetup(PhantomData)
    }

    /// Queue the factory that resolves `C` for the boot. `None` loads from the
    /// environment over `C::defaults()`; `Some(cfg)` makes `cfg` the **base**
    /// the environment overlays — what an app passes to
    /// `Module::for_root(config)`. Every configurable module's `for_root` routes
    /// through this.
    ///
    /// **Both arms resolve through [`Config::resolve`]**, which is what makes
    /// the override per field. `Some` used to call
    /// [`ContainerBuilder::provide`], registering the struct verbatim and making
    /// every `NESTRS_<NS>__*` variable in that namespace inert: pinning a port
    /// with `..Default::default()` silently froze the fourteen other HTTP
    /// fields, so a deployment setting `NESTRS_HTTP__PORT` or
    /// `NESTRS_HTTP__TLS_CERT_FILE` got nothing and no warning. A test's seed
    /// still wins over either arm — a seed short-circuits the factory.
    ///
    /// **They differ in one way, and only one: precedence in the queue.** A
    /// pinned base is a *declaration*
    /// ([`provide_declared_factory`](ContainerBuilder::provide_declared_factory)),
    /// so it supersedes the environment-only factory a bare import of the same
    /// module queues, wherever the two fall in `imports = [..]`. Without that,
    /// `imports = [AudioModule, StorageModule::for_root(cfg)]` dropped `cfg`
    /// silently, because `AudioModule` imports `StorageModule` and got there
    /// first. Two pinned bases for one config fail the boot naming it
    /// ([`ContestedDeclarationError`](nest_rs_core::ContestedDeclarationError))
    /// rather than letting import order decide.
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
    /// nothing else — `WsModule` and `StorageModule` today:
    ///
    /// ```ignore
    /// pub type WsSetup = ConfigSetup<WsModule, WsConfig>;
    ///
    /// impl WsModule {
    ///     pub fn for_root(config: impl Into<Option<WsConfig>>) -> WsSetup {
    ///         ConfigModule::setup(config)
    ///     }
    /// }
    /// ```
    ///
    /// Lives on `ConfigModule` rather than as `ConfigSetup::new` so the setup
    /// type keeps **no public method of its own** — the *one seam, one value*
    /// rule holds for a shared setup exactly as for a hand-written one. A module
    /// whose `collect` queues more than the config, or whose `register` does
    /// more than recurse, still writes its own type.
    pub fn setup<M: Module, C: Config>(pinned: impl Into<Option<C>>) -> ConfigSetup<M, C> {
        ConfigSetup {
            pinned: pinned.into(),
            module: PhantomData,
        }
    }
}

/// The [`DynamicModule`] behind a `for_root` that only pins a config: resolves
/// `C` (environment over the pinned base, per field) in the factory phase, then
/// registers `M`'s ordinary wiring. The pinned base supersedes the plain env
/// factory `M` itself queues via [`for_feature`](ConfigModule::for_feature),
/// wherever the two fall in `imports = [..]`.
///
/// Built by [`ConfigModule::setup`]; it deliberately exposes nothing else.
pub struct ConfigSetup<M, C> {
    pinned: Option<C>,
    module: PhantomData<fn() -> M>,
}

impl<M: Module, C: Config> DynamicModule for ConfigSetup<M, C> {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        <M as Module>::register(builder)
    }
}

/// The import produced by [`ConfigModule::for_feature`]. Queues a factory that
/// loads and validates `C` in the factory phase, as global infrastructure.
pub struct ConfigFeatureSetup<C>(PhantomData<fn() -> C>);

impl<C: Config> DynamicModule for ConfigFeatureSetup<C> {
    // Loading is sync-but-fallible and `register` cannot return an error, so
    // we queue a factory the build awaits — an Err there aborts boot with the
    // variable named.
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature::<C>(None, builder)
    }
}

/// The import produced by [`ConfigModule::for_root`]. Registers the active
/// [`Environment`] so later config loads see the resolved `.env` cascade.
pub struct ConfigRootSetup;

impl DynamicModule for ConfigRootSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        // `Environment::from_env` reads `NESTRS_ENV` from the real process env;
        // dotenv values reach config reads lazily via `env_var` (the in-crate
        // map), so collect mutates no process state — no `set_var` on the boot
        // path that a spawned worker's `getenv` could race.
        builder.provide(Environment::from_env())
    }
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    /// The `for_root` / `for_feature` seam, exercised through a real boot.
    ///
    /// A nested module rather than a second file-level `#[cfg(test)] mod`:
    /// `CLAUDE.md` fixes the shape at one per file — "`#[cfg(test)] mod tests`
    /// in the file under test" — and a `seam::…` filter path existed nowhere
    /// else in either workspace, so "where is this asserted?" had two answers
    /// inside one file.
    mod seam {
        use nest_rs_core::{App, ContainerBuilder, DynamicModule, Module, module};
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
                    bucket: env.get("BUCKET").unwrap_or(base.bucket),
                })
            }
        }

        /// Stands in for a framework module that registers its own config unpinned.
        struct OwnerModule;

        impl Module for OwnerModule {
            fn register(builder: ContainerBuilder) -> ContainerBuilder {
                builder
            }
            fn collect(builder: ContainerBuilder) -> ContainerBuilder {
                ConfigModule::for_feature::<SeamConfig>().collect(builder)
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

        /// The bug this replaced: factories are first-queued-wins, so a bare import
        /// listed above the pin — the shape you get for free when another module
        /// imports the same one — dropped the pinned value on the floor with no
        /// warning. Both orders must now yield the pin.
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

        /// Two pins cannot both win, and picking one by position is the silent drop
        /// under another name — so the boot fails naming the config.
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
        }

        /// `App::new` runs `register` but never `collect`, so a queued factory would
        /// never be built. It used to boot anyway and leave the config missing —
        /// readable only as a `None` at first use, far from the cause.
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
    use nest_rs_core::Container;

    /// `ConfigModule::for_root()`'s collect registers the active [`Environment`]
    /// and **nothing else** — in particular it does not merge the `.env` cascade
    /// into the process environment. Pinned because the docs used to claim the
    /// opposite (that building any default-source reader, or importing
    /// `for_root`, published the cascade under a `Once`), and the advice that
    /// followed from it — "a hermetic test must avoid `for_namespace` /
    /// `ConfigModule::for_root()`" — sent readers away from calls that are
    /// side-effect-free. `Environment::init` is the one publisher, and it is
    /// tested where it lives.
    #[test]
    fn for_root_collect_does_not_publish_the_cascade_into_the_process_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                &format!(
                    "{}=from_dotenv",
                    var_name("for_root_guard", "SHOULD_STAY_UNSET"),
                ),
            )?;

            let builder = ConfigModule::for_root().collect(Container::builder());
            let container = builder.build();

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
    fn a_config_read_sees_the_cascade_without_publishing_it() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                &format!("{}=from_dotenv", var_name("readpath_guard", "URL")),
            )?;

            // Asserted unconditionally. It was guarded on `value.is_some()`,
            // reasoning that "the cascade is parsed once per process, so a
            // sibling test may have frozen it against a different working
            // directory" — but the runner is nextest, which gives every test its
            // own process, so the `OnceLock` is always this test's. The guard
            // made the read half of the cell unfailable, which is worse than
            // empty (`testing.md` clause 3).
            let value = crate::ConfigService::for_namespace("readpath_guard").get("URL");
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

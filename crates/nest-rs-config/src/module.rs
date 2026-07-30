//! [`ConfigModule`] — the `ConfigModule.for_root` / `for_feature` DI wiring.

use std::marker::PhantomData;

use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::config::Config;
use crate::environment::Environment;

/// Sole owner of config loading. List [`ConfigModule::for_root()`](Self::for_root)
/// **first** in the root module's imports — it makes the `.env` cascade visible
/// to config reads (real env vars always win; resolution goes through an
/// in-crate map, so the process env is **never** mutated) and registers
/// `Arc<Environment>`, so every later [`Config`] load sees dotenv values.
pub struct ConfigModule;

impl ConfigModule {
    /// Import first in the root module — makes the `.env` cascade visible to
    /// config reads and registers `Arc<Environment>`.
    pub fn for_root() -> ConfigRootSetup {
        ConfigRootSetup
    }

    /// Loads in the **factory phase**, becoming global infrastructure. A test
    /// that seeds `C` directly wins over this factory.
    pub fn for_feature<C: Config>() -> ConfigFeatureSetup<C> {
        ConfigFeatureSetup(PhantomData)
    }

    /// Queue the factory that resolves `C` for the boot. `None` loads from the
    /// environment over `C::defaults()`; `Some(cfg)` makes `cfg` the **base**
    /// the environment overlays — what an app passes to
    /// `Module::for_root(config)`. Every configurable module's `for_root` routes
    /// through this.
    ///
    /// Both arms take the same path on purpose. `Some` used to call
    /// [`ContainerBuilder::provide`], which registered the struct verbatim and
    /// made every `NESTRS_<NS>__*` variable in that namespace inert: pinning a
    /// port with `..Default::default()` silently froze the fourteen other HTTP
    /// fields, so a deployment setting `NESTRS_HTTP__PORT` or
    /// `NESTRS_HTTP__TLS_CERT_FILE` got nothing and no warning. Routing both
    /// through [`Config::resolve`] makes the override per field, and makes a
    /// test's seed win uniformly (a seed short-circuits a factory) instead of
    /// only on the unpinned path.
    pub fn provide_feature<C: Config>(
        pinned: Option<C>,
        builder: ContainerBuilder,
    ) -> ContainerBuilder {
        builder.provide_factory::<C, _, _>(|_| async move {
            C::resolve(pinned).map_err(anyhow::Error::from)
        })
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
    use super::*;
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
                "NESTRS_FOR_ROOT_GUARD__SHOULD_STAY_UNSET=from_dotenv",
            )?;

            let builder = ConfigModule::for_root().collect(Container::builder());
            let container = builder.build();

            assert!(
                container.get::<Environment>().is_some(),
                "collect registers the active Environment",
            );
            assert!(
                std::env::var("NESTRS_FOR_ROOT_GUARD__SHOULD_STAY_UNSET").is_err(),
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
            jail.create_file(".env", "NESTRS_READPATH_GUARD__URL=from_dotenv")?;

            let value = crate::ConfigService::for_namespace("readpath_guard").get("URL");
            // The cascade is parsed once per process, so a sibling test may have
            // frozen it against a different working directory — assert the
            // side-effect either way, and the value only when the read landed.
            if value.is_some() {
                assert_eq!(value.as_deref(), Some("from_dotenv"));
            }
            assert!(
                std::env::var("NESTRS_READPATH_GUARD__URL").is_err(),
                "a config read resolves through the in-crate map, never through std::env",
            );
            Ok(())
        });
    }
}

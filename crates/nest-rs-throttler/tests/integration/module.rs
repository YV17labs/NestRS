//! The default limit `ThrottlerModule` falls back to (`src/module.rs`).

use std::time::Duration;

use nest_rs_throttler::DEFAULT_THROTTLE;

#[test]
fn default_throttle_constant_is_60_per_minute() {
    // A change here re-tunes every rate-limited route.
    assert_eq!(DEFAULT_THROTTLE.limit(), 60);
    assert_eq!(DEFAULT_THROTTLE.window(), Duration::from_secs(60));
}

/// The policy is a dependency, never a default: a guard built by any path but
/// `ThrottlerModule` fails the boot naming what it missed, and told that import.
mod guard_outside_for_root {
    use std::sync::Arc;

    use nest_rs_core::{App, AppBuilder, MissingDependencyError, injectable, module};
    use nest_rs_guards::GlobalGuards;
    use nest_rs_throttler::{
        InMemoryThrottler, Throttle, ThrottlerGuard, ThrottlerModule, ThrottlerStore,
    };

    /// The dependency `app` is refused on, having been told to import the port.
    async fn dependency_told_the_import(app: AppBuilder) -> String {
        let Err(refused) = app.build().await else {
            panic!("a guard with no policy must not boot");
        };
        let missing = refused
            .downcast_ref::<MissingDependencyError>()
            .unwrap_or_else(|| panic!("a named unmet dependency, got: {refused:#}"));
        assert!(
            missing
                .remedy
                .as_deref()
                .is_some_and(|remedy| remedy.contains("`nest_rs::throttler::ThrottlerModule`")),
            "{missing}"
        );
        missing.dependency.to_string()
    }

    #[module(providers = [ThrottlerGuard])]
    struct GuardAsProviderModule;

    #[tokio::test]
    async fn a_guard_listed_in_providers_without_for_root_fails_the_boot_naming_the_policy() {
        let dependency = dependency_told_the_import(
            App::builder()
                .provide_dyn::<dyn ThrottlerStore>(Arc::new(InMemoryThrottler::new()))
                .module::<GuardAsProviderModule>(),
        )
        .await;
        assert!(dependency.contains("Throttle"), "{dependency}");
    }

    #[tokio::test]
    async fn a_guard_listed_by_hand_over_a_seeded_policy_fails_the_boot_naming_its_store() {
        let dependency = dependency_told_the_import(
            App::builder()
                .provide(Throttle::per_minute(5))
                .module::<GuardAsProviderModule>(),
        )
        .await;
        assert!(dependency.contains("CountingStore"), "{dependency}");
    }

    #[module(providers = [GlobalGuards<(ThrottlerGuard,)>])]
    struct PoolWithoutThePort;

    #[tokio::test]
    async fn a_root_listing_the_guard_in_its_pool_with_no_port_imported_is_told_the_import() {
        let dependency =
            dependency_told_the_import(App::builder().module::<PoolWithoutThePort>()).await;
        assert!(dependency.contains("Throttle"), "{dependency}");
    }

    #[expect(
        dead_code,
        reason = "the guard is declared for the container to resolve, never read"
    )]
    #[injectable]
    struct BindsTheGuard {
        #[inject]
        guard: Arc<ThrottlerGuard>,
    }

    #[module(providers = [BindsTheGuard])]
    struct AdapterWithoutThePort;

    /// The pool reports the guard on its root's behalf, and importing that root
    /// from its own adapter is no fix.
    #[tokio::test]
    async fn an_adapter_beside_a_root_listing_the_guard_is_told_the_port_not_the_root() {
        let Err(refused) = App::builder()
            .module::<AdapterWithoutThePort>()
            .module::<PoolWithoutThePort>()
            .build()
            .await
        else {
            panic!("no module builds the guard");
        };
        let said = refused.to_string();
        assert!(
            said.contains("depends on `ThrottlerGuard`")
                && said.contains("`nest_rs::throttler::ThrottlerModule`"),
            "{said}"
        );
        assert!(!said.contains("— add `"), "{said}");
    }

    #[expect(
        dead_code,
        reason = "the store is declared for the container to resolve, never read"
    )]
    #[injectable]
    struct ReadsTheStore {
        #[inject]
        store: Arc<dyn ThrottlerStore>,
    }

    #[module(providers = [ReadsTheStore])]
    struct StoreWithoutThePort;

    /// `store_the_app_binds` below links a module listing the store, one this app
    /// does not reach, so the access graph says it: the crate's line, all the same.
    /// With no module listing it, `tests/unlisted` holds the unmet dependency's.
    #[tokio::test]
    async fn a_provider_injecting_the_store_with_no_port_imported_is_told_the_import() {
        let Err(refused) = App::builder().module::<StoreWithoutThePort>().build().await else {
            panic!("no module binds the store");
        };
        let said = refused.to_string();
        assert!(
            said.contains("depends on `dyn ThrottlerStore`")
                && said.contains("`nest_rs::throttler::ThrottlerModule`"),
            "{said}"
        );
    }

    #[module(imports = [ThrottlerModule], providers = [GlobalGuards<(ThrottlerGuard,)>])]
    struct PoolOverThePort;

    #[tokio::test]
    async fn a_root_importing_the_port_for_its_pool_boots_with_the_guard_the_port_built() {
        let app = App::builder()
            .module::<PoolOverThePort>()
            .build()
            .await
            .expect("the import is what the refusal told it to write");
        assert!(app.container().get::<ThrottlerGuard>().is_some());
    }
}

/// A store the app binds itself, whose module is the fix however the port's
/// crate names its own: importing the port beside it registers the key twice.
mod store_the_app_binds {
    use std::sync::Arc;

    use nest_rs_core::{
        AccessGraphError, App, ContainerBuilder, Module, Registering, async_trait, injectable,
        module,
    };
    use nest_rs_throttler::{
        Decision, Throttle, ThrottlerModule, ThrottlerStore, ThrottlerStoreError,
    };

    #[injectable]
    #[derive(Default)]
    struct SharedStore;

    #[async_trait]
    impl ThrottlerStore for SharedStore {
        async fn hit(&self, _key: &str, _limit: Throttle) -> Result<Decision, ThrottlerStoreError> {
            Ok(Decision::allowed())
        }
    }

    #[module(providers = [SharedStore as dyn ThrottlerStore])]
    struct SharedStoreModule;

    #[injectable]
    struct Reader {
        #[inject]
        store: Arc<dyn ThrottlerStore>,
    }

    #[module(providers = [Reader])]
    struct ReaderWithoutTheStore;

    #[module(imports = [SharedStoreModule, ReaderWithoutTheStore])]
    struct ListedBesideTheReader;

    #[tokio::test]
    async fn a_reader_beside_the_module_listing_the_store_is_told_that_module() {
        let Err(refused) = App::builder()
            .module::<ListedBesideTheReader>()
            .build()
            .await
        else {
            panic!("the reader imports nothing");
        };
        let violation = refused
            .downcast_ref::<AccessGraphError>()
            .unwrap_or_else(|| panic!("not an access violation: {refused:#}"));
        assert_eq!(violation.owner, "SharedStoreModule");
        let said = violation.to_string();
        assert!(said.contains("add `SharedStoreModule`"), "{said}");
        assert!(!said.contains("ThrottlerModule"), "{said}");
    }

    #[module(imports = [SharedStoreModule], providers = [Reader])]
    struct ReaderOverTheStore;

    #[tokio::test]
    async fn following_that_sentence_boots_on_the_apps_store() {
        let app = App::builder()
            .module::<ReaderOverTheStore>()
            .build()
            .await
            .unwrap_or_else(|refused| panic!("the import is the fix: {refused:#}"));
        let container = app.container();
        let read = container.get::<Reader>().expect("the reader is built");
        let bound = container.get::<SharedStore>().expect("the store is listed");
        assert_eq!(
            Arc::as_ptr(&read.store).cast::<()>(),
            Arc::as_ptr(&bound).cast::<()>(),
        );
    }

    /// The port's sentence, followed where the port is the wrong fix.
    #[module(imports = [ThrottlerModule], providers = [Reader])]
    struct ReaderOverThePort;

    #[module(imports = [SharedStoreModule, ReaderOverThePort])]
    struct PortBesideTheStore;

    #[tokio::test]
    async fn the_port_beside_a_listed_store_is_refused_as_the_wrong_fix() {
        let Err(refused) = App::builder().module::<PortBesideTheStore>().build().await else {
            panic!("one key, two registrations");
        };
        let said = format!("{refused:#}");
        assert!(said.contains("duplicate provider"), "{said}");
    }

    /// The same store a hand-written module registers, while `SharedStoreModule`
    /// above is only another binary's here.
    struct HandWrittenStoreModule;

    impl Module for HandWrittenStoreModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide_dyn::<dyn ThrottlerStore>(Arc::new(SharedStore))
        }
    }

    #[module(imports = [ReaderWithoutTheStore, HandWrittenStoreModule])]
    struct RegisteredAfterTheReader;

    #[tokio::test]
    async fn a_reader_built_before_a_hand_written_store_is_told_that_module() {
        let Err(refused) = App::builder()
            .module::<RegisteredAfterTheReader>()
            .build()
            .await
        else {
            panic!("the store registers after its reader");
        };
        let said = format!("{refused:#}");
        assert!(said.contains("`HandWrittenStoreModule`"), "{said}");
        assert!(!said.contains("ThrottlerModule"), "{said}");
    }
}

mod pseudonym_key {
    use std::sync::Arc;

    use nest_rs_core::{App, Collecting, ContainerBuilder, Module, Registering, module};
    use nest_rs_throttler::{
        BACKEND_REMEDY, Decision, Throttle, ThrottlerConfig, ThrottlerGuard, ThrottlerModule,
        ThrottlerStore,
    };

    /// A store outside the process, as a vendor's would be: the trait's default.
    struct Elsewhere;

    #[async_trait::async_trait]
    impl ThrottlerStore for Elsewhere {
        async fn hit(
            &self,
            _key: &str,
            _limit: Throttle,
        ) -> Result<Decision, nest_rs_throttler::ThrottlerStoreError> {
            Ok(Decision::allowed())
        }
    }

    struct ElsewhereModule;

    impl Module for ElsewhereModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }

        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_declared_factory::<Arc<dyn ThrottlerStore>, _, _>(
                BACKEND_REMEDY,
                |_| async { Ok(Arc::new(Elsewhere) as Arc<dyn ThrottlerStore>) },
            )
        }
    }

    #[module(imports = [ThrottlerModule, ElsewhereModule])]
    struct KeylessModule;

    #[module(imports = [
        ThrottlerModule::for_root(ThrottlerConfig {
            pseudonym_key: Some("thirty-two bytes, exactly enough".into()),
            ..ThrottlerConfig::default()
        }),
        ElsewhereModule,
    ])]
    struct KeyedModule;

    #[tokio::test]
    async fn a_store_outside_the_process_boots_only_with_a_key() {
        let Err(refused) = App::builder().module::<KeylessModule>().build().await else {
            panic!("a store outside the process must not boot without a pseudonym key");
        };
        let said = format!("{refused:#}");
        assert!(
            said.contains(&nest_rs_config::var_name("throttler", "PSEUDONYM_KEY"))
                && said.contains("Elsewhere"),
            "{said}"
        );
        let app = App::builder()
            .module::<KeyedModule>()
            .build()
            .await
            .expect("the same store boots with the key");
        assert!(app.container().get::<ThrottlerGuard>().is_some());
    }

    #[module(providers = [ThrottlerGuard])]
    struct HandWiredModule;

    #[tokio::test]
    async fn a_guard_built_by_the_container_never_counts_in_a_bare_store() {
        let built = App::builder()
            .provide(Throttle::per_minute(5))
            .provide_dyn::<dyn ThrottlerStore>(Arc::new(Elsewhere))
            .module::<HandWiredModule>()
            .build()
            .await;
        assert!(built.is_err(), "a guard outside for_root must not boot");
    }
}

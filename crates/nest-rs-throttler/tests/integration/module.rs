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

    #[tokio::test]
    async fn a_provider_injecting_the_store_with_no_port_imported_is_told_the_import() {
        let dependency =
            dependency_told_the_import(App::builder().module::<StoreWithoutThePort>()).await;
        assert!(dependency.contains("ThrottlerStore"), "{dependency}");
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

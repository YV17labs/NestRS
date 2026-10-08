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
/// `ThrottlerModule::for_root` fails the boot naming `Throttle`.
mod guard_outside_for_root {
    use std::sync::Arc;

    use nest_rs_core::{App, MissingDependencyError, module};
    use nest_rs_throttler::{InMemoryThrottler, ThrottlerGuard, ThrottlerStore};

    #[module(providers = [ThrottlerGuard])]
    struct GuardAsProviderModule;

    #[tokio::test]
    async fn a_guard_listed_in_providers_without_for_root_fails_the_boot_naming_the_policy() {
        let err = match App::builder()
            .provide_dyn::<dyn ThrottlerStore>(Arc::new(InMemoryThrottler::new()))
            .module::<GuardAsProviderModule>()
            .build()
            .await
        {
            Ok(_) => panic!("a guard with no policy must not boot"),
            Err(e) => e,
        };
        let missing = err
            .downcast_ref::<MissingDependencyError>()
            .unwrap_or_else(|| panic!("a named unmet dependency, got: {err}"));
        assert!(
            missing.dependency.contains("Throttle"),
            "the missing dependency is the policy: {missing}",
        );
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
        async fn hit(&self, _key: &str, _limit: Throttle) -> Decision {
            Decision::allowed()
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

    #[module(imports = [ThrottlerModule::for_root(None), ElsewhereModule])]
    struct KeylessModule;

    #[module(imports = [
        ThrottlerModule::for_root(ThrottlerConfig {
            pseudonym_key: Some("thirty-two bytes, exactly enough".to_owned()),
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

//! The default limit `ThrottlerModule` falls back to (`src/module.rs`).

use std::time::Duration;

use nest_rs_throttler::DEFAULT_THROTTLE;

#[test]
fn default_throttle_constant_is_60_per_minute() {
    // App code reads `ThrottlerConfig.limit.unwrap_or(DEFAULT_THROTTLE.limit)` —
    // a silent change here re-tunes every rate-limited route.
    assert_eq!(DEFAULT_THROTTLE.limit, 60);
    assert_eq!(DEFAULT_THROTTLE.window, Duration::from_secs(60));
}

/// The policy is a dependency, never a default: a guard built from the
/// container by any path but `ThrottlerModule::for_root` — `providers =
/// [ThrottlerGuard]` beside a vendor store, no `for_root` — must fail the boot
/// naming the missing `Throttle`, not run 60/minute over a limit the operator
/// configured as 1. It did the latter for one audit round, because `default`
/// was a plain field the `#[injectable]` constructor filled with
/// `Default::default()`.
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

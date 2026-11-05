//! `RedisThrottlerModule`'s composition, in process: registered by an importer
//! that never collects it, the store it would bind is named rather than absent;
//! imported at the root, it supersedes the in-process store an adapter binding
//! the guard brings, wherever the two fall in `imports`.

use std::sync::Arc;

use nest_rs_core::{App, Module, module};
use nest_rs_redis::RedisThrottlerModule;
use nest_rs_throttler::{ThrottlerConfig, ThrottlerGuard, ThrottlerModule, ThrottlerStore};

use crate::connection::DemotedRedis;

#[tokio::test]
async fn the_binding_registered_without_its_collect_fails_the_boot_naming_its_store() {
    assert_eq!(
        crate::registered_alone::<RedisThrottlerModule>().await,
        std::any::type_name::<Arc<dyn ThrottlerStore>>()
    );
}

/// What an adapter whose controller binds `ThrottlerGuard` imports.
#[module(imports = [ThrottlerModule])]
struct AdapterBindingTheGuard;

#[module(imports = [AdapterBindingTheGuard, RedisThrottlerModule])]
struct AdapterThenBinding;

#[module(imports = [RedisThrottlerModule, AdapterBindingTheGuard])]
struct BindingThenAdapter;

/// The store `M`'s app counts through, over `server`.
async fn store_name<M: Module>(server: &DemotedRedis) -> &'static str {
    let app = App::builder()
        .provide(server.config())
        .provide(ThrottlerConfig {
            pseudonym_key: Some("nest-rs-redis integration pseudonym key".into()),
            ..ThrottlerConfig::default()
        })
        .module::<M>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("{} boots: {refused:#}", std::any::type_name::<M>()));
    assert!(
        app.container().get::<ThrottlerGuard>().is_some(),
        "the adapter's import registers the guard"
    );
    let store = app
        .container()
        .get_dyn::<dyn ThrottlerStore>()
        .expect("a store is bound");
    store.name()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_binding_supersedes_the_store_an_adapter_brings_in_either_order() {
    let server = DemotedRedis::start().await;
    for name in [
        store_name::<AdapterThenBinding>(&server).await,
        store_name::<BindingThenAdapter>(&server).await,
    ] {
        assert!(name.contains("RedisThrottler"), "{name}");
    }
}

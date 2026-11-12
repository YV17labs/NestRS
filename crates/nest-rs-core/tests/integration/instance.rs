//! Covers `src/instance.rs` — a value an app seeds under a marker is a type of
//! its own: two values of one type under two markers stay apart, and a provider
//! injects each by its `Instanced` type.

use std::sync::Arc;

use nest_rs_core::{App, Instanced, injectable, instance, module};

#[instance]
struct Upstream;

#[instance]
struct Downstream;

struct Client(&'static str);

#[injectable]
struct Relay {
    #[inject]
    upstream: Arc<Instanced<Upstream, Client>>,
    #[inject]
    downstream: Arc<Instanced<Downstream, Client>>,
}

#[module(providers = [Relay])]
struct RelayModule;

#[tokio::test]
async fn two_values_of_one_type_seeded_under_two_markers_are_injected_apart() {
    let app = App::builder()
        .provide(Instanced::<Upstream, _>::from(Client("upstream")))
        .provide(Instanced::<Downstream, _>::from(Client("downstream")))
        .module::<RelayModule>()
        .build()
        .await
        .expect("a seed is app-global, so a module importing nothing injects it");

    let relay = app.container().get::<Relay>().expect("the module built it");
    assert_eq!(relay.upstream.0, "upstream");
    assert_eq!(relay.downstream.0, "downstream");
}

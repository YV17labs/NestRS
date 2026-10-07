//! `nest-rs-redis`'s suite in process, without a Redis: [`connection`] for the
//! boot against a scripted server and the budget's place below the ports' nets,
//! [`tls`] for a certificate refused at the handshake, and [`queue`] and
//! [`schedule`] and [`throttler`] for what their bindings' composition refuses
//! before Redis is dialled. What only a live Redis shows is in `e2e`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod connection;
#[path = "../harness/mod.rs"]
mod harness;
mod queue;
mod schedule;
mod throttler;
mod tls;

use nest_rs_core::{App, ContainerBuilder, Imported, LateFactoryError, Module};

/// The type the boot names when a hand-written importer imports `M` in its
/// register alone: what `M`'s `collect` queues, refused as late rather than
/// never built.
async fn registered_alone<M: Module + 'static>() -> &'static str {
    struct RegistersOnly<N>(std::marker::PhantomData<N>);

    impl<N: Module> Module for RegistersOnly<N> {
        fn register(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
            builder.import::<N>()
        }
    }

    let Err(refused) = App::builder().module::<RegistersOnly<M>>().build().await else {
        panic!("what the binding queues in `collect` would never be built");
    };
    refused
        .downcast_ref::<LateFactoryError>()
        .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"))
        .type_name
}

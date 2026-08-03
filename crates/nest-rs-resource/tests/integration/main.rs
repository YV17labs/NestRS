//! Compile-time guard: wire-only `#[expose]` must not pull in `async_graphql`.
//! Run with `cargo test -p nest-rs-resource --no-default-features`.

mod exposures;
#[cfg(feature = "graphql")]
mod graphql;

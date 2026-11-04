//! `#[expose]`: the wire surface it always emits, and the GraphQL one under the
//! `graphql` flag. A decorator's expansion is tested in its surface crate, so
//! this folder mirrors the macro crate's `src/`, `nest-rs-seaorm-macros`.

mod active;
mod dto;
#[cfg(feature = "graphql")]
mod input;
mod lifecycle;
#[cfg(feature = "graphql")]
mod relations;
mod wire;

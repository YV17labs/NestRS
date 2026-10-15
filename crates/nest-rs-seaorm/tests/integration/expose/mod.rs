//! `#[expose]` and its enum mode `#[wire_enum]`: the wire surface they always
//! emit, and the GraphQL one under the `graphql` flag. A decorator's expansion
//! is tested in its surface crate, so this folder mirrors the macro crate's
//! `src/`, `nest-rs-seaorm-macros`.

#[cfg(feature = "graphql")]
mod input;
mod lifecycle;
#[cfg(feature = "graphql")]
mod relations;
mod wire;
mod wire_enum;

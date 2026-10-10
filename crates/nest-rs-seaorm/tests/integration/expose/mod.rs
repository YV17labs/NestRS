//! `#[expose]` and its enum mode `#[wire_enum]`: the wire surface they always
//! emit, and the GraphQL one under the `graphql` flag.

#[cfg(feature = "graphql")]
mod graphql;
#[cfg(feature = "graphql")]
mod relations;
mod wire;
mod wire_enum;

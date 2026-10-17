//! `#[wire_enum]` — the enum mode of `#[expose]`: its four derives and their
//! `crate = ` overrides resolve against the umbrella alone.

use nest_rs::seaorm::wire_enum;

/// The wire-only arm, without the GraphQL `Enum` derive.
#[wire_enum]
#[serde(rename_all = "snake_case")]
pub enum HygieneWireTier {
    Free,
    PayAsYouGo,
}

/// The GraphQL arm: async-graphql roots `Enum` at the call site's manifest, so
/// this fails the day the `crate = ` override is dropped.
#[cfg(feature = "graphql")]
#[wire_enum(graphql)]
pub enum HygieneStage {
    Draft,
    Shipped,
}

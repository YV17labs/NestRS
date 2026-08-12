//! What the exposure decorators emit, and what they refuse.
//!
//! `exposures` / `graphql` / `wire_enum` are compile-time guards on the
//! emission — chiefly that a wire-only `#[expose]` must not pull in
//! `async_graphql`, which is what `cargo test -p nest-rs-resource
//! --no-default-features` checks. `diagnostics` is the other half: a trybuild
//! snapshot per refusal, since the sentence a developer reads is as much the
//! decorator's contract as the code it writes.

mod diagnostics;
mod exposures;
#[cfg(feature = "graphql")]
mod graphql;
mod wire_enum;

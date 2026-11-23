//! `nest-rs-macro-hygiene`'s suite: what the decorators refuse, pinned through
//! the one dependency a developer installs, and what a build lacking a
//! capability still compiles as 7.0.0 compiled it.

mod diagnostics;
#[cfg(all(feature = "graphql", not(feature = "resilience")))]
mod resolver;

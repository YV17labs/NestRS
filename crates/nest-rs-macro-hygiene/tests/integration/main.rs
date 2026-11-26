//! `nest-rs-macro-hygiene`'s suite: what the decorators refuse, pinned through
//! the one dependency a developer installs, and an operation's attributes of
//! the app's own, each kept on the method it was written on.

mod diagnostics;
#[cfg(feature = "graphql")]
mod resolver;

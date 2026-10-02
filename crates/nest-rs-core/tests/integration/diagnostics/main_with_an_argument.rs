//! `#[main]` takes no arguments, and refuses one naming why.
//!
//! The runtime it builds is the one an app's transports need, sized by the
//! deployment, and torn down within the shutdown budget: an argument has
//! nothing left to choose. The refusal names that rather than calling the key
//! unknown, because a developer arriving from `#[tokio::main]` writes `flavor`
//! expecting it to mean what it meant there.

#[nest_rs_core::main(flavor = "current_thread")]
async fn main() {}

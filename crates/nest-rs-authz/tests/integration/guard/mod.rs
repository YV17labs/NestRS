//! Mirror tests for `src/guard.rs`; each file covers one of the guard's four
//! entries and is gated on the feature that compiles it.

#[cfg(feature = "http")]
mod http;

#[cfg(feature = "mcp")]
mod mcp;

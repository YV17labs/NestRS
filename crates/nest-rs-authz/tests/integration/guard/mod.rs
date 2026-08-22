//! Mirror tests for `src/guard.rs` — the one `AbilityGuard`, which answers every
//! transport, so its tests sit beside it at the suite root rather than under the
//! edge that happens to exercise them. Each file covers one of the guard's four
//! entries and is gated on the same feature that compiles it.

#[cfg(feature = "http")]
mod http;

#[cfg(feature = "mcp")]
mod mcp;

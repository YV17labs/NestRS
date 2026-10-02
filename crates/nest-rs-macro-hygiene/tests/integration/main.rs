//! Suite root: the static half of this crate's witness.
//!
//! `src/` proves hygiene by *compiling* a consumer that declares one
//! dependency — decisive, but only for the decorators a zero-dep crate can
//! reach. `#[crud]`/`#[expose]` cannot live there (they need a real entity and
//! a real service), and the crate doc used to record that their routing "rests
//! on review". It no longer does: [`emissions`] reads every `*-macros` source
//! and fails on a path rooted outside the framework, whichever decorator emits
//! it and whether or not a consumer here can call it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod emissions;

//! The scaffold, actually compiled.
//!
//! `nestrs new` and `nestrs g` write Rust into someone else's repository. The
//! `integration` suite reads that output back and asserts on its **text** —
//! which catches a wrong dependency or a missing feature, and cannot catch a
//! template that emits code the compiler rejects. Only the user would find that.
//!
//! So this suite runs the generator and then runs `cargo check` over what it
//! produced. It is `e2e` because it is minutes, not milliseconds: it resolves a
//! real dependency graph and builds the framework from source.

mod scaffold;

//! `scope` written bare names the key, not the grammar.
//!
//! `nest_rs_codegen::needs_a_value` exists for exactly this — "a bare
//! `expected `=`` names the grammar and not the key" — and this decorator
//! reached syn instead.

use nest_rs_core::injectable;

#[injectable(scope)]
#[derive(Default)]
struct Bare;

fn main() {}

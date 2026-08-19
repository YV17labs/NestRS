//! `#[injectable]` declares one residency.
//!
//! It parsed one argument and let `Parser::parse2`'s full-consumption
//! requirement report the rest, so a repeat died on syn's "unexpected token"
//! naming neither the key nor the fact — on the struct half of all five
//! `on_provider` pairs and the most-written decorator in either workspace.
//! Dropping one of two residencies by source order is the provider's whole
//! lifecycle decided by which came second.

use nest_rs_core::injectable;

#[injectable(scope = request, scope = transient)]
#[derive(Default)]
struct Ambiguous;

fn main() {}

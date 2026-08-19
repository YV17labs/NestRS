//! A bare `#[crud]` key names the key, not the grammar.
//!
//! `expected \`=\`` is syn's answer and it names neither the decorator nor the
//! key the developer wrote. `nest_rs_codegen::args::needs_a_value` words that
//! refusal once; it had exactly one adopter — the job family — while every other
//! `key = value` grammar fell through to the bare token error.

use nest_rs_http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity)]
impl OrgsController {}

fn main() {}

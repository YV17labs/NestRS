//! The unknown-key half of the grammar every `key = value` decorator owes.
//! Its sibling `controller_path_without_a_value` pins the bare-key half, and
//! both route through the one `nest_rs_codegen::unmatched_meta` sentence — so
//! the refusal names the offending key *and* lists the alternatives in
//! declaration order.

use nest_rs_http::controller;

#[controller(path = "/widgets", prefix = "/v1")]
pub struct WidgetsController;

fn main() {}

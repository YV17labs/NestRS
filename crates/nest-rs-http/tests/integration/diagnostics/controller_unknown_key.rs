//! The unknown-key refusal every `key = value` decorator owes, the one each
//! decorator keeps a snapshot of: issued by `nest_rs_codegen::Grammar`, whose
//! unit tests hold the bare-key and repeated-key halves for every grammar — so
//! the refusal names the offending key *and* lists the alternatives in
//! declaration order.

use nest_rs_http::controller;

#[controller(path = "/widgets", prefix = "/v1")]
pub struct WidgetsController;

fn main() {}

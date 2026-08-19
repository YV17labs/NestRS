//! The fifth refusal a `key = value` grammar owes — a required key not written
//! at all. Eight sites in six crates wrote it in three verbs and three shapes,
//! six of them spanned at the item rather than the declaration;
//! `nest_rs_codegen::missing_argument` is the one wording, and this pins it.

use nest_rs_http::controller;

#[controller]
pub struct WidgetsController;

fn main() {}

//! A non-string `path` names the decorator and the key.
//!
//! There were two sentences for this one question: the shared
//! `nest_rs_codegen::require_str_lit`, which names both, and a second helper
//! answering `expected a string literal` at seven call sites — naming neither.
//! The sharpest instance was `version`, whose parser threads a decorator name
//! through every refusal it words itself and delegated this one.

use nest_rs_http::controller;

#[controller(path = 42)]
struct UsersController;

fn main() {}

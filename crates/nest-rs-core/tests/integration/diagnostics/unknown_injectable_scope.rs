//! `#[injectable(scope = …)]` accepts only `singleton`/`request`/`transient` —
//! a typo is a spanned compile error, not a silent default.

use nest_rs_core::injectable;

#[injectable(scope = bogus)]
struct Svc;

fn main() {}

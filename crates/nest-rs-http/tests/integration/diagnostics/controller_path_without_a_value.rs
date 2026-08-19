//! A bare key names the key, not the grammar — and is not called unknown.
//!
//! Every accepting arm of a `Punctuated<Meta, _>` grammar is guarded
//! `Meta::NameValue(nv) if nv.path.is_ident(…)`, so a bare `path` is a
//! `Meta::Path` and falls to the unknown-key arm. Adopting the shared
//! unknown-argument sentence there printed *"unknown #[controller] argument
//! `path`; expected `path` or `version`"* — a sentence declaring the key
//! unknown and then listing it, which is worse than the bare `expected \`=\``
//! it replaced.

use nest_rs_http::controller;

#[controller(path)]
struct UsersController;

fn main() {}

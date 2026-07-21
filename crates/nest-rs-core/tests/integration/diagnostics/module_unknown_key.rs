//! `#[module]` rejects an unknown argument key (e.g. a NestJS-style `exports`,
//! which nestrs deliberately does not have) with a spanned error naming the
//! valid keys, never silently ignoring it.

use nest_rs_core::module;

struct Foo;

#[module(exports = [Foo])]
struct BadModule;

fn main() {}

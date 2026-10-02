//! `#[main]` goes on a function. On any other item it says so, naming the
//! shape it expects, rather than rustc's "expected `fn`" from inside the
//! expansion.

#[nest_rs_core::main]
struct Entry;

fn main() {}

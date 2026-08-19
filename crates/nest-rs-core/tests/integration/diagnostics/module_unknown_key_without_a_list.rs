//! An unknown `#[module]` key is refused by name whatever value shape follows.
//!
//! The parser read `= [` before judging the key, so the shared sentence was
//! reachable only when the wrong key happened to take a bracketed value: the
//! sibling fixture writes `exports = [Foo]` and got the name, while this shape
//! answered `expected square brackets`. The snapshot pinned the reachable half,
//! so the join read green over a refusal that fired on one value shape.

use nest_rs_core::module;

#[module(porviders = Foo)]
struct AppModule;

fn main() {}

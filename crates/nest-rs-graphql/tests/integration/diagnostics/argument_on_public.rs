//! `#[public]` is a flag: its presence *is* the declaration, so an argument on
//! it is a compile error rather than something the expansion drops.
//!
//! It sits beside `#[authorize(Action, Entity)]`, which does take arguments, so
//! writing `#[public(read_only)]` by analogy is the plausible mistake — and
//! before the refusal it produced an ungated, unmasked operation with the
//! compiler silent. `#[public]` is one of the three greppable sites `CLAUDE.md`
//! reserves for the authn/authz decision, which is why it is the flag this
//! snapshot is written on.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public(read_only)]
    async fn ping(&self) -> i32 {
        0
    }
}

fn main() {}

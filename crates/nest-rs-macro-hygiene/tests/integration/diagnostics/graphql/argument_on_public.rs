//! `#[public]` alone is a posture, and `#[public(Action, Subject)]` names what
//! it reads. A lone word beside it — `read_only`, plausible as a flag — is neither,
//! so it is refused naming what the attribute takes, never dropped into an
//! ungated, unmasked operation with the compiler silent.

use nest_rs::graphql::{operations, resolver};

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

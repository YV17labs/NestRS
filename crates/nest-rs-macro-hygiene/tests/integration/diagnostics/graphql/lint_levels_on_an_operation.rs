//! A lint level written on an operation holds the method it was written on,
//! whose body the developer wrote: `#[operations]` leaves it there, plain or
//! under a `#[cfg_attr]`. A `#[deny]` refuses what its body trips, an
//! expectation the body meets says nothing, and one it does not meet is
//! reported at the method — the field generated beside it carries none.

#![deny(unfulfilled_lint_expectations)]

use nest_rs::graphql::{operations, resolver};

#[resolver]
struct DepotResolver;

#[operations]
impl DepotResolver {
    #[query]
    #[public]
    #[deny(unused_variables)]
    async fn crates(&self) -> i32 {
        let counted = 3;
        4
    }

    #[query]
    #[public]
    #[cfg_attr(all(), expect(unused_variables, reason = "the bay narrows the count next release"))]
    async fn pallets(&self, bay: String) -> i32 {
        5
    }

    #[query]
    #[public]
    #[cfg_attr(all(), expect(unused_variables))]
    async fn docks(&self) -> i32 {
        6
    }
}

fn main() {}

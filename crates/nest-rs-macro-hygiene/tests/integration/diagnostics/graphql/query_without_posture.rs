//! A `#[query]` with neither `#[authorize(...)]` nor `#[public]` must not
//! compile — an operation the developer forgot to think about never ships
//! ungated and unmasked.

use nest_rs::graphql::{resolver, operations};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    async fn ping(&self) -> i32 {
        0
    }
}

fn main() {}

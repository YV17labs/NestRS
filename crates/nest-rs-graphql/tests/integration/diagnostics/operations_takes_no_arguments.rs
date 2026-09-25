//! The impl half collects; it declares nothing. The sentence names the
//! operations it collects, read off the same `DecoratorPair` the wrong-shape
//! error reads, so adding a role cannot leave one of the two listing the old
//! set.
//!
//! The refusal is the only error: the impl stays, so its method is still found
//! by its caller and the host still answers the module that lists it.

use nest_rs_core::module;
use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations(path = "/graphql")]
impl DemoResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        1
    }
}

#[module(providers = [DemoResolver])]
struct DemoModule;

fn main() {
    let _ = DemoResolver.ping();
}

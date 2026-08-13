//! The impl half collects; it declares nothing. The sentence names the
//! operations it collects, read off the same `DecoratorPair` the wrong-shape
//! error reads, so adding a role cannot leave one of the two listing the old
//! set.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations(path = "/graphql")]
impl DemoResolver {}

fn main() {}

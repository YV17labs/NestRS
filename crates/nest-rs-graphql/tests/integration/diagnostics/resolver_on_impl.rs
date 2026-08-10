//! One decorator, one item shape: `#[resolver]` names the struct only. Reaching
//! for it on the impl block must name the sibling that does belong there, rather
//! than report the shape it happened to expect.

use nest_rs_graphql::resolver;

#[resolver]
struct DemoResolver;

#[resolver]
impl DemoResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }
}

fn main() {}

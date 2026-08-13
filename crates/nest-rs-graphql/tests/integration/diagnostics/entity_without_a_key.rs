//! The arguments *are* the `@key`. With none there is nothing for a router to
//! match a reference against, and async-graphql's own refusal lands on the
//! `#[operations]` attribute naming a generated type the developer never wrote.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity]
    #[public]
    async fn find_widget(&self) -> nest_rs_graphql::async_graphql::Result<i32> {
        Ok(0)
    }
}

fn main() {}

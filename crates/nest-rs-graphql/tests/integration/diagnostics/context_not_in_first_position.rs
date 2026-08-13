//! async-graphql's `#[Object]` recognises the context parameter only directly
//! after `&self`. A later one is read as a **schema argument**, which fails as
//! an `InputType` bound on `&ContextBase<…>` — a type the developer never wrote
//! — and on an `#[entity]` also joins the `@key` a router matches references
//! against. The macro knew the rule (it inserts at position 1) and handled only
//! the *absent* case.

use nest_rs_graphql::async_graphql::SimpleObject;
use nest_rs_graphql::{operations, resolver};

#[derive(SimpleObject)]
struct Widget {
    id: i32,
}

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(
        &self,
        id: i32,
        ctx: &nest_rs_graphql::async_graphql::Context<'_>,
    ) -> nest_rs_graphql::async_graphql::Result<Widget> {
        let _ = ctx;
        Ok(Widget { id })
    }
}

fn main() {}

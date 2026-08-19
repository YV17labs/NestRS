//! Two `#[authorize(...)]` on one operation is refused, not resolved.
//!
//! GraphQL parses its own posture grammar (it accepts `bind` and `id_arg`, which
//! no other edge can express) and shares this one sentence with the other three.

use nest_rs_graphql::{operations, resolver};

struct Read;
struct Update;
mod users {
    pub struct Entity;
}

#[resolver]
struct UsersResolver;

#[operations]
impl UsersResolver {
    #[query]
    #[authorize(Read, users::Entity)]
    #[authorize(Update, users::Entity)]
    async fn users(&self) -> nest_rs_graphql::async_graphql::Result<String> {
        Ok(String::new())
    }
}

fn main() {}

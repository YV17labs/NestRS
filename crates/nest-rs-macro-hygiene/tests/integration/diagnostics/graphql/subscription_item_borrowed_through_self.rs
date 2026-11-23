//! A subscription's stream may borrow through a lifetime its method declares,
//! but async-graphql's derive reads the stream's item outside the method, where
//! that lifetime is undeclared: `Item = &'a str` is rustc's `E0261`, the
//! lifetime deliberately failing to resolve there. The stream yields owned
//! items instead, borrowing what it reads them from.

use nest_rs::graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs::graphql::{operations, resolver};

#[resolver]
#[derive(Default)]
struct ChimesResolver {
    chimes: Vec<String>,
}

#[operations]
impl ChimesResolver {
    #[subscription]
    #[public]
    async fn chimes<'a>(&'a self) -> impl Stream<Item = &'a str> + 'a {
        stream::iter(self.chimes.iter().map(String::as_str))
    }
}

fn main() {}

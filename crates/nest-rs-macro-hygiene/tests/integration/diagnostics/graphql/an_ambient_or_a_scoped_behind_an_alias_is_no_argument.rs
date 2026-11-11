//! A resolver reads `Ambient<T>` and `Scoped<T>` by their names and leaves them
//! out of the schema's arguments. Behind an alias either would be an argument
//! the client sends, so each is refused naming the fix, beside async-graphql's
//! own refusal of it as an input.

use nest_rs::core::{Ambient, AmbientValue, Scoped};
use nest_rs::graphql::{operations, resolver};

#[derive(Clone)]
struct Caller;

impl AmbientValue for Caller {}

struct Cart;

type Me = Ambient<Caller>;

type MyCart = Scoped<Cart>;

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn me(&self, _caller: Me, _cart: MyCart) -> i32 {
        0
    }
}

fn main() {}

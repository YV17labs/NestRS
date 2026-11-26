//! Rust's `#[deprecated]` on an operation. It deprecates the method for Rust
//! callers — the field `#[operations]` generates is the only one — while a
//! client reads a field's deprecation off the schema; the field's own form is
//! named.

use nest_rs::graphql::{operations, resolver};

#[resolver]
struct PostsResolver;

#[operations]
impl PostsResolver {
    #[query]
    #[public]
    #[deprecated(since = "7.1.0", note = "ask `posts`")]
    async fn feed(&self) -> String {
        String::new()
    }
}

fn main() {}

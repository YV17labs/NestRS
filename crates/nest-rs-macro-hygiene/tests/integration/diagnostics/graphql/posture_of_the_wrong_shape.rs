//! A posture that is not `#[word(Action, Subject)]` is refused in the grammar's
//! sentence, whatever stops the parse. syn's `expected parentheses` and
//! `expected ,` name neither the form nor what it takes.

use nest_rs::graphql::{operations, resolver};

struct Read;

mod posts {
    pub struct Entity;
}

#[resolver]
struct ValueOnPublic;

#[operations]
impl ValueOnPublic {
    #[query]
    #[public = "x"]
    async fn list(&self) -> i32 {
        0
    }
}

#[resolver]
struct NoCommaOnPublic;

#[operations]
impl NoCommaOnPublic {
    #[query]
    #[public(Read posts::Entity)]
    async fn list(&self) -> i32 {
        0
    }
}

#[resolver]
struct ValueOnAuthorize;

#[operations]
impl ValueOnAuthorize {
    #[query]
    #[authorize = "x"]
    async fn list(&self) -> i32 {
        0
    }
}

#[resolver]
struct NoCommaOnAuthorize;

#[operations]
impl NoCommaOnAuthorize {
    #[query]
    #[authorize(Read posts::Entity)]
    async fn list(&self) -> i32 {
        0
    }
}

#[resolver]
struct BareAuthorize;

#[operations]
impl BareAuthorize {
    #[query]
    #[authorize]
    async fn list(&self) -> i32 {
        0
    }
}

fn main() {}

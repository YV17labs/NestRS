//! A posture that is not `#[word(Action, Subject)]` is refused in the grammar's
//! sentence, whatever stops the parse. syn's `expected parentheses` and
//! `expected ,` name neither the form nor what it takes.

use nest_rs::ws::{gateway, messages};

struct Read;

mod posts {
    pub struct Entity;
}

#[gateway(path = "/value")]
struct ValueOnPublic;

#[messages]
impl ValueOnPublic {
    #[subscribe_message("list")]
    #[public = "x"]
    async fn list(&self) -> i32 {
        0
    }
}

#[gateway(path = "/comma")]
struct NoCommaOnPublic;

#[messages]
impl NoCommaOnPublic {
    #[subscribe_message("list")]
    #[public(Read posts::Entity)]
    async fn list(&self) -> i32 {
        0
    }
}

#[gateway(path = "/bare")]
struct BareAuthorize;

#[messages]
impl BareAuthorize {
    #[subscribe_message("list")]
    #[authorize]
    async fn list(&self) -> i32 {
        0
    }
}

fn main() {}

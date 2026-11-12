//! A posture that is not `#[word(Action, Subject)]` is refused in the grammar's
//! sentence, whatever stops the parse. syn's `expected parentheses` and
//! `expected ,` name neither the form nor what it takes.

use nest_rs::http::{controller, routes};

struct Read;

mod posts {
    pub struct Entity;
}

#[controller(path = "/value")]
struct ValueOnPublic;

#[routes]
impl ValueOnPublic {
    #[get("/")]
    #[public = "x"]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/comma")]
struct NoCommaOnPublic;

#[routes]
impl NoCommaOnPublic {
    #[get("/")]
    #[public(Read posts::Entity)]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/bare")]
struct BareAuthorize;

#[routes]
impl BareAuthorize {
    #[get("/")]
    #[authorize]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

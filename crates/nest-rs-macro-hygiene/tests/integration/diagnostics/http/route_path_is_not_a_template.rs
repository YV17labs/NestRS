//! The ways of writing a template's braces wrong, each refused at the literal
//! with what to write instead: a parameter is named, opened and closed, and a
//! literal brace is doubled.

use nest_rs::http::{controller, routes};

#[controller(path = "/unnamed")]
struct UnnamedController;

#[routes]
impl UnnamedController {
    #[get("/{}")]
    async fn read(&self) -> String {
        String::new()
    }
}

#[controller(path = "/unclosed")]
struct UnclosedController;

#[routes]
impl UnclosedController {
    #[get("/{id")]
    async fn read(&self) -> String {
        String::new()
    }
}

#[controller(path = "/unopened")]
struct UnopenedController;

#[routes]
impl UnopenedController {
    #[get("/id}")]
    async fn read(&self) -> String {
        String::new()
    }
}

#[controller(path = "/bare")]
struct BareController;

#[routes]
impl BareController {
    #[get("/items/:")]
    async fn read(&self) -> String {
        String::new()
    }
}

fn main() {}

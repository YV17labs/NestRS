//! A verb's one argument is the route's path, read as a string literal. Anything
//! else — a bare word, or no argument at all — is refused at what was written,
//! naming the verb as every value refusal names its site; syn's `expected string
//! literal` named neither the verb nor what it takes.

use nest_rs_http::{controller, routes};

#[controller(path = "/a")]
struct AController;

#[routes]
impl AController {
    #[get(users)]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/b")]
struct BController;

#[routes]
impl BController {
    #[post]
    #[public]
    async fn create(&self) -> String {
        String::new()
    }
}

fn main() {}

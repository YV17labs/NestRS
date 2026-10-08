//! Rust's `#[deprecated]` on the controller struct deprecates a type no client
//! calls. Read nowhere, it would leave every route undeprecated in the document
//! and on the wire, so `#[controller]` refuses it and names the route's form.

use nest_rs::http::{controller, routes};

#[controller(path = "/posts")]
#[deprecated(since = "7.1.0")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

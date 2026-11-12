//! Rust's `#[deprecated]` on a route handler. It deprecates the method for Rust
//! callers — the framework is the only one — and clippy holds its `since` to a
//! crate version, while a client reads a date; the route's own form is named.

use nest_rs::http::{controller, routes};

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    #[public]
    #[deprecated(since = "7.1.0", note = "use `GET /v2/posts`")]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

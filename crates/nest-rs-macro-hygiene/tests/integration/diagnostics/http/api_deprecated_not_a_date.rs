//! `#[api(deprecated)]` written as a version. The `Deprecation` header (RFC
//! 9745) a client reads is a date, so the key takes an RFC 3339 full-date.

use nest_rs::http::{controller, routes};

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    #[public]
    #[api(deprecated = "7.1")]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

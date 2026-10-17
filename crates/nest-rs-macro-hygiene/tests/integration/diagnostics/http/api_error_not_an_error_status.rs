//! `#[api(error(…))]` given a success status. The key documents the bodies a
//! handler answers failures with; the success payload is `response = T`.

use nest_rs::http::{controller, routes};

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    #[public]
    #[api(error(200 = String))]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

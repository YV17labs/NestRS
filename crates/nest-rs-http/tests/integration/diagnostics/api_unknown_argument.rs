//! `#[api]` given a key it does not take. The family's sentence names the key as
//! written and lists the ones `#[api]` does take, as every other `key = value`
//! decorator's does.

use nest_rs_http::{controller, routes};

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    #[public]
    #[api(summry = "List users")]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

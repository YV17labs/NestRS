//! `#[api]` given a key with no value. The family's sentence names the key and
//! the edit that completes it, where syn said only `expected =`.

use nest_rs_http::{controller, routes};

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    #[public]
    #[api(summary)]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

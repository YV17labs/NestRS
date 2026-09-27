//! `#[api]` given one key twice. Accepting it would publish whichever
//! `summary` came last and drop the other without a word — prose the document
//! carries instead of a doc comment, replaced by source order.

use nest_rs_http::{controller, routes};

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    #[public]
    #[api(summary = "List users", summary = "List every user")]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
